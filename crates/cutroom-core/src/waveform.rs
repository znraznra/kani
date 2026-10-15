//! Audio waveforms for the timeline: peak data, how it is turned into pixel
//! columns for drawing, and the cache file format. Pure; the GStreamer side
//! that decodes the audio lives in the GTK crate.

/// Peaks stored per second of audio (one per 10 ms).
pub const PEAKS_PER_SECOND: u32 = 100;

const MAGIC: &[u8; 5] = b"CRWF1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peaks {
    pub per_second: u32,
    /// Largest absolute sample in each slice, 0 (silence) to 255 (full scale).
    pub data: Vec<u8>,
}

impl Peaks {
    pub fn duration_seconds(&self) -> f64 {
        if self.per_second == 0 {
            0.0
        } else {
            self.data.len() as f64 / f64::from(self.per_second)
        }
    }

    /// One height (0..=255) per pixel column: the loudest peak in the span the
    /// column covers, so zooming out never hides a loud moment. Columns
    /// outside the audio are 0.
    pub fn columns(&self, start_seconds: f64, seconds_per_column: f64, columns: usize) -> Vec<u8> {
        let per_second = f64::from(self.per_second);
        (0..columns)
            .map(|c| {
                let from = (start_seconds + c as f64 * seconds_per_column) * per_second;
                let to = from + seconds_per_column * per_second;
                let first = from.floor().max(0.0) as usize;
                // Always look at least one peak, even when zoomed in far.
                let last = (to.ceil().max(from.floor() + 1.0)) as usize;
                self.data
                    .get(first..last.min(self.data.len()))
                    .and_then(|slice| slice.iter().copied().max())
                    .unwrap_or(0)
            })
            .collect()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAGIC.len() + 8 + self.data.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.per_second.to_le_bytes());
        out.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// `None` for anything that isn't a complete, current-format file.
    pub fn decode(bytes: &[u8]) -> Option<Peaks> {
        let rest = bytes.strip_prefix(MAGIC)?;
        let per_second = u32::from_le_bytes(rest.get(0..4)?.try_into().ok()?);
        let count = u32::from_le_bytes(rest.get(4..8)?.try_into().ok()?) as usize;
        let data = rest.get(8..)?;
        (per_second > 0 && data.len() == count).then(|| Peaks {
            per_second,
            data: data.to_vec(),
        })
    }
}

/// Builds peaks from decoded mono samples as they arrive in chunks.
pub struct PeakBuilder {
    samples_per_peak: usize,
    in_slice: usize,
    loudest: f32,
    data: Vec<u8>,
}

impl PeakBuilder {
    pub fn new(sample_rate: u32) -> PeakBuilder {
        PeakBuilder {
            samples_per_peak: (sample_rate / PEAKS_PER_SECOND).max(1) as usize,
            in_slice: 0,
            loudest: 0.0,
            data: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for sample in samples {
            self.loudest = self.loudest.max(sample.abs());
            self.in_slice += 1;
            if self.in_slice == self.samples_per_peak {
                self.flush_slice();
            }
        }
    }

    fn flush_slice(&mut self) {
        self.data
            .push((self.loudest.min(1.0) * 255.0).round() as u8);
        self.in_slice = 0;
        self.loudest = 0.0;
    }

    pub fn finish(mut self) -> Peaks {
        if self.in_slice > 0 {
            self.flush_slice();
        }
        Peaks {
            per_second: PEAKS_PER_SECOND,
            data: self.data,
        }
    }
}

/// How tall to draw a peak: a gentle curve so quiet passages stay visible.
pub fn display_height(peak: u8, max_height: f64) -> f64 {
    (f64::from(peak) / 255.0).powf(0.6) * max_height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peaks(values: &[u8]) -> Peaks {
        Peaks {
            per_second: 100,
            data: values.to_vec(),
        }
    }

    #[test]
    fn the_builder_takes_the_loudest_sample_of_each_slice() {
        let mut b = PeakBuilder::new(1000); // 10 samples per peak
        let mut samples = vec![0.0f32; 30];
        samples[3] = -0.5; // slice 0
        samples[15] = 1.0; // slice 1
        samples[29] = 0.25; // slice 2
        b.push(&samples[..12]); // chunks may split slices
        b.push(&samples[12..]);
        let p = b.finish();
        assert_eq!(p.per_second, 100);
        assert_eq!(p.data, vec![128, 255, 64]);
    }

    #[test]
    fn a_partial_last_slice_still_counts_and_loud_samples_clamp() {
        let mut b = PeakBuilder::new(1000);
        b.push(&[0.0, 0.0, 3.0]); // clipped sample above full scale
        assert_eq!(b.finish().data, vec![255]);
    }

    #[test]
    fn duration_follows_the_peak_count() {
        assert!((peaks(&[0; 250]).duration_seconds() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn columns_take_the_loudest_peak_in_each_span() {
        let p = peaks(&[10, 200, 30, 40, 250, 60, 70, 80]);
        // 2 peaks per column.
        assert_eq!(p.columns(0.0, 0.02, 4), vec![200, 40, 250, 80]);
        // Zoomed in: several columns share one peak.
        assert_eq!(p.columns(0.0, 0.005, 4), vec![10, 10, 200, 200]);
    }

    #[test]
    fn columns_start_inside_the_audio_and_run_off_its_end_quietly() {
        let p = peaks(&[10, 20, 30, 40]);
        assert_eq!(p.columns(0.02, 0.01, 4), vec![30, 40, 0, 0]);
        assert_eq!(p.columns(5.0, 0.01, 2), vec![0, 0], "entirely past the end");
        assert_eq!(
            p.columns(-0.01, 0.01, 2),
            vec![0, 10],
            "before the start is silent"
        );
    }

    #[test]
    fn the_cache_format_round_trips_and_rejects_damage() {
        let p = peaks(&[1, 2, 3, 250]);
        assert_eq!(Peaks::decode(&p.encode()), Some(p.clone()));
        let mut cut = p.encode();
        cut.pop();
        assert_eq!(Peaks::decode(&cut), None, "truncated");
        assert_eq!(Peaks::decode(b"nonsense"), None);
        let mut other = p.encode();
        other[0] = b'X';
        assert_eq!(Peaks::decode(&other), None, "wrong magic");
        assert_eq!(Peaks::decode(&[]), None);
    }

    #[test]
    fn heights_are_zero_for_silence_and_full_for_full_scale() {
        assert_eq!(display_height(0, 20.0), 0.0);
        assert!((display_height(255, 20.0) - 20.0).abs() < 1e-9);
        assert!(
            display_height(64, 20.0) > 20.0 * 64.0 / 255.0,
            "quiet sounds are lifted"
        );
    }
}
