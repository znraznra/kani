//! Getting Kani started safely on any machine.
//!
//! * **Bundled prerequisites.** When `kani.exe` sits in the `bin` folder of a
//!   packaged Kani (made by `tools/windows/package.ps1`), GStreamer, GTK and
//!   the image loaders are pointed at the copies inside that folder, so it
//!   runs the same wherever it is, whatever else is installed.
//! * **Safe mode.** Kani leaves a marker file while it runs and removes it when
//!   it closes properly. If the marker is still there at the next start (the
//!   program, or the whole computer, went down), that start is in safe mode:
//!   no hardware video decoding, no graphics-chip compositing, no automatic
//!   graphics-card check, and GTK draws with the processor. `KANI_SAFE=1` or
//!   Settings > Advanced > Start in safe mode does the same on purpose.
//! * **Start-up trace.** Each step of start-up (and the risky things done
//!   later: decoders, encoder and graphics tests) is written to
//!   `logs/startup.log` in the settings folder and flushed to disk at once, so
//!   after a hard freeze the last line says how far it got.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

static SAFE: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Option<(File, Instant)>> = Mutex::new(None);
static LOCK: Mutex<Option<PathBuf>> = Mutex::new(None);
static REASON: Mutex<Option<String>> = Mutex::new(None);

/// Is this run in safe mode?
pub fn safe() -> bool {
    SAFE.load(Ordering::Relaxed)
}

/// Why this run is in safe mode, for the status bar.
pub fn safe_reason() -> Option<String> {
    REASON.lock().ok().and_then(|r| r.clone())
}

/// Write one start-up step to the trace (and the console), flushed to disk.
pub fn step(what: &str) {
    eprintln!("kani: [start] {what}");
    if let Ok(mut guard) = TRACE.lock() {
        if let Some((file, began)) = guard.as_mut() {
            let _ = writeln!(file, "{:>8.3} s  {what}", began.elapsed().as_secs_f64());
            let _ = file.flush();
            let _ = file.sync_all();
        }
    }
}

/// The packaged layout: `<root>/bin/kani.exe`, `<root>/lib/gstreamer-1.0`.
pub fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if !bin
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
    {
        return None;
    }
    let root = bin.parent()?;
    root.join("lib")
        .join("gstreamer-1.0")
        .is_dir()
        .then(|| root.to_path_buf())
}

fn set_if_unset(key: &str, value: &Path) {
    if std::env::var_os(key).is_none() {
        std::env::set_var(key, value);
    }
}

/// Point GStreamer, GTK and the image loaders at a packaged Kani's own copies.
fn use_bundle(root: &Path, data: &Path) {
    let lib = root.join("lib");
    let plugins = lib.join("gstreamer-1.0");
    // Only the bundled plug-ins: a different GStreamer elsewhere on the
    // machine must not be mixed in.
    std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &plugins);
    std::env::set_var("GST_PLUGIN_PATH_1_0", &plugins);
    let scanner = root
        .join("libexec")
        .join("gstreamer-1.0")
        .join("gst-plugin-scanner.exe");
    if scanner.is_file() {
        std::env::set_var("GST_PLUGIN_SCANNER_1_0", &scanner);
    }
    set_if_unset(
        "GST_REGISTRY_1_0",
        &data.join(format!("gst-registry-{}.bin", env!("CARGO_PKG_VERSION"))),
    );
    let share = root.join("share");
    set_if_unset("XDG_DATA_DIRS", &share);
    set_if_unset(
        "GSETTINGS_SCHEMA_DIR",
        &share.join("glib-2.0").join("schemas"),
    );
    // Image loaders (the SVG icons need one): list them once per version.
    if let Some(loaders) = std::fs::read_dir(lib.join("gdk-pixbuf-2.0"))
        .ok()
        .and_then(|mut d| d.next())
        .and_then(|e| e.ok())
        .map(|e| e.path().join("loaders"))
        .filter(|p| p.is_dir())
    {
        let cache = data.join(format!("loaders-{}.cache", env!("CARGO_PKG_VERSION")));
        if !cache.is_file() {
            let query = root.join("bin").join("gdk-pixbuf-query-loaders.exe");
            let dlls: Vec<PathBuf> = std::fs::read_dir(&loaders)
                .map(|d| {
                    d.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
                        .collect()
                })
                .unwrap_or_default();
            if let Ok(out) = std::process::Command::new(&query).args(&dlls).output() {
                let _ = std::fs::write(&cache, out.stdout);
            }
        }
        if cache.is_file() {
            set_if_unset("GDK_PIXBUF_MODULE_FILE", &cache);
        }
        set_if_unset("GDK_PIXBUF_MODULEDIR", &loaders);
    }
}

/// Run first thing in `main`, before GStreamer or GTK start.
pub fn prepare(config_dir: Option<&Path>, safe_setting: bool) {
    let data = config_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(data.join("logs"));
    if let Ok(file) = File::create(data.join("logs").join("startup.log")) {
        if let Ok(mut t) = TRACE.lock() {
            *t = Some((file, Instant::now()));
        }
    }
    step(&format!(
        "Kani {} on {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    if let Ok(exe) = std::env::current_exe() {
        match bundle_root(&exe) {
            Some(root) => {
                use_bundle(&root, &data);
                step(&format!(
                    "using the prerequisites bundled in {}",
                    root.display()
                ));
            }
            None => step(&format!(
                "not a packaged copy ({}): GTK and GStreamer come from the system",
                exe.display()
            )),
        }
    }
    let lock = data.join("running.lock");
    let crashed = lock.is_file();
    let forced = std::env::var("KANI_SAFE").is_ok_and(|v| v != "0") || safe_setting;
    let reason = if crashed {
        Some("Kani did not close properly last time, so it started in safe mode".to_string())
    } else if forced {
        Some("safe mode is on".to_string())
    } else {
        None
    };
    if let Some(r) = &reason {
        SAFE.store(true, Ordering::Relaxed);
        // GTK draws with the processor: no graphics driver involved.
        set_if_unset("GSK_RENDERER", Path::new("cairo"));
        step(&format!(
            "SAFE MODE ({r}): no hardware decoding, no graphics-chip compositing, no automatic graphics-card check, GTK drawn by the processor"
        ));
        if let Ok(mut g) = REASON.lock() {
            *g = Some(r.clone());
        }
    }
    let _ = std::fs::write(&lock, format!("{}\n", std::process::id()));
    if let Ok(mut g) = LOCK.lock() {
        *g = Some(lock);
    }
}

/// Kani is closing properly: the next start is a normal one.
pub fn clean_exit() {
    step("closed properly");
    if let Some(lock) = LOCK.lock().ok().and_then(|mut g| g.take()) {
        let _ = std::fs::remove_file(lock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packaged_copy_is_recognised_by_its_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Kani");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin").join("kani.exe");
        assert_eq!(bundle_root(&exe), None, "no plug-in folder yet");
        std::fs::create_dir_all(root.join("lib").join("gstreamer-1.0")).unwrap();
        assert_eq!(bundle_root(&exe), Some(root.clone()));
        assert_eq!(
            bundle_root(&dir.path().join("target/release/kani.exe")),
            None
        );
    }
}
