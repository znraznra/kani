use serde::{Deserialize, Serialize};
use std::ops::{Add, Neg, Sub};

/// Timeline resolution: 254,016,000,000 ticks per second.
///
/// Chosen so that common video frame rates (including the NTSC "1001" rates)
/// and common audio sample rates all land on whole numbers of ticks, which
/// avoids drift and rounding errors when cutting frame-accurately.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// A point or span on the timeline, in [`TICKS_PER_SECOND`] units.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Ticks(pub i64);

impl Ticks {
    pub const ZERO: Ticks = Ticks(0);

    pub fn from_seconds_f64(seconds: f64) -> Self {
        Ticks((seconds * TICKS_PER_SECOND as f64).round() as i64)
    }

    pub fn as_seconds_f64(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }

    pub fn abs(self) -> Ticks {
        Ticks(self.0.abs())
    }

    /// Exact conversion from a frame count. `None` if the rate does not divide
    /// the tick base evenly or the result overflows.
    pub fn from_frames(frames: i64, rate: FrameRate) -> Option<Ticks> {
        rate.ticks_per_frame()?.checked_mul(frames).map(Ticks)
    }

    /// Index of the frame containing this time (rounds toward negative infinity).
    pub fn frame_floor(self, rate: FrameRate) -> Option<i64> {
        Some(self.0.div_euclid(rate.ticks_per_frame()?))
    }
}

impl Add for Ticks {
    type Output = Ticks;
    fn add(self, rhs: Ticks) -> Ticks {
        Ticks(self.0 + rhs.0)
    }
}

impl Sub for Ticks {
    type Output = Ticks;
    fn sub(self, rhs: Ticks) -> Ticks {
        Ticks(self.0 - rhs.0)
    }
}

impl Neg for Ticks {
    type Output = Ticks;
    fn neg(self) -> Ticks {
        Ticks(-self.0)
    }
}

/// Rational frame rate, e.g. 30000/1001 for 29.97 fps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    pub const FPS_24: FrameRate = FrameRate::new(24, 1);
    pub const FPS_25: FrameRate = FrameRate::new(25, 1);
    pub const FPS_30: FrameRate = FrameRate::new(30, 1);
    pub const FPS_50: FrameRate = FrameRate::new(50, 1);
    pub const FPS_60: FrameRate = FrameRate::new(60, 1);
    pub const NTSC_23_976: FrameRate = FrameRate::new(24000, 1001);
    pub const NTSC_29_97: FrameRate = FrameRate::new(30000, 1001);
    pub const NTSC_59_94: FrameRate = FrameRate::new(60000, 1001);

    pub const fn new(num: u32, den: u32) -> Self {
        FrameRate { num, den }
    }

    /// Whole ticks in one frame, or `None` if it isn't an integer.
    pub fn ticks_per_frame(self) -> Option<i64> {
        if self.num == 0 || self.den == 0 {
            return None;
        }
        let total = TICKS_PER_SECOND.checked_mul(self.den as i64)?;
        let num = self.num as i64;
        (total % num == 0).then(|| total / num)
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_rates_are_exact() {
        for rate in [
            FrameRate::FPS_24,
            FrameRate::FPS_25,
            FrameRate::FPS_30,
            FrameRate::FPS_50,
            FrameRate::FPS_60,
            FrameRate::NTSC_23_976,
            FrameRate::NTSC_29_97,
            FrameRate::NTSC_59_94,
        ] {
            assert!(rate.ticks_per_frame().is_some(), "{rate:?} is not exact");
        }
    }

    #[test]
    fn frame_roundtrip_is_lossless() {
        let rate = FrameRate::NTSC_29_97;
        for frame in [0, 1, 29, 30, 1000, 107_892] {
            let t = Ticks::from_frames(frame, rate).unwrap();
            assert_eq!(t.frame_floor(rate), Some(frame));
        }
    }

    #[test]
    fn audio_sample_rates_divide_evenly() {
        for hz in [44_100i64, 48_000, 96_000] {
            assert_eq!(TICKS_PER_SECOND % hz, 0, "{hz} Hz does not divide evenly");
        }
    }

    #[test]
    fn invalid_rates_are_rejected() {
        assert_eq!(FrameRate::new(0, 1).ticks_per_frame(), None);
        assert_eq!(FrameRate::new(30, 0).ticks_per_frame(), None);
        assert_eq!(FrameRate::new(11, 1).ticks_per_frame(), None);
    }
}
