//! Test support: stub ffmpeg, ffprobe and upscaler programs that log every call and create the
//! files a real run would. Shared by the engine tests and the golden test against the scripts.
#![cfg(test)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::engine::Binaries;
use super::plan::{JobSpec, Variant};

/// One stub program for all three tools: `stub.py TOOL STATE_DIR ARGS...`. It appends the call to
/// `STATE_DIR/calls.txt` (a `CALL tool` line, then one argument per line) and behaves according to
/// `STATE_DIR/config.json`. Per-segment facts (frame count, duration) are keyed by the segment
/// file's base name, so they hold whether the file is written in `segments/` or in `seg_part/`.
pub const STUB_PY: &str = include_str!("../../tests/support/stub.py");

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("vhs-gui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the stub reports and which faults it injects.
#[derive(Clone)]
pub struct StubCfg {
    pub duration: f64,
    pub fps: &'static str,
    pub w: u32,
    pub h: u32,
    pub audio: bool,
    pub frames: u32,
    pub upscaler_short_by: u32,
    pub rebuild_short_by: u32,
    /// `(segment file name, duration)`: the first encode of that segment reports this duration.
    pub dur_override_once: Vec<(&'static str, f64)>,
}

impl Default for StubCfg {
    fn default() -> Self {
        Self {
            duration: 70.0,
            fps: "30000/1001",
            w: 720,
            h: 480,
            audio: true,
            frames: 4,
            upscaler_short_by: 0,
            rebuild_short_by: 0,
            dur_override_once: Vec::new(),
        }
    }
}

impl StubCfg {
    fn json(&self) -> String {
        let over: Vec<String> = self
            .dur_override_once
            .iter()
            .map(|(n, d)| format!("\"{n}\": {d}"))
            .collect();
        format!(
            "{{\"duration\": {}, \"fps\": \"{}\", \"w\": {}, \"h\": {}, \"audio\": {}, \"frames\": {}, \
             \"upscaler_short_by\": {}, \"rebuild_short_by\": {}, \"dur_override_once\": {{{}}}}}",
            self.duration,
            self.fps,
            self.w,
            self.h,
            self.audio,
            self.frames,
            self.upscaler_short_by,
            self.rebuild_short_by,
            over.join(", ")
        )
    }
}

pub struct Kit {
    pub dir: TempDir,
    pub state: PathBuf,
    pub bins: Binaries,
    pub spec: JobSpec,
}

/// One shared launcher and stub for every test, written once and never rewritten while in use, so
/// executing it cannot hit "Text file busy" (a fork in another test thread inheriting a write
/// handle). Each test gets symlinks to the launcher; the launcher finds that test's state directory
/// from the path it was invoked by (`bin/<tool>` next to `state/`).
fn shared_launcher() -> &'static Path {
    use std::sync::OnceLock;
    static LAUNCHER: OnceLock<PathBuf> = OnceLock::new();
    LAUNCHER.get_or_init(|| {
        let dir = std::env::temp_dir().join("vhs-gui-test-stubs");
        fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("stub.py");
        let launcher = dir.join("launcher");
        let launcher_text = format!(
            "#!/bin/sh\nexec python3 '{}' \"$(basename \"$0\")\" \"$(dirname \"$0\")/../state\" \"$@\"\n",
            stub.display()
        );
        for (path, text, mode) in [(&stub, STUB_PY, 0o644), (&launcher, launcher_text.as_str(), 0o755)] {
            // Rewrite only when the content changed, via an atomic rename.
            if fs::read_to_string(path).ok().as_deref() != Some(text) {
                let tmp = dir.join(format!(".tmp-{}", std::process::id()));
                fs::write(&tmp, text).unwrap();
                fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)).unwrap();
                fs::rename(&tmp, path).unwrap();
            }
        }
        launcher
    })
}

impl Kit {
    pub fn new(name: &str, variant: Variant, cfg: &StubCfg) -> Self {
        let dir = TempDir::new(name);
        let state = dir.0.join("state");
        let bin = dir.0.join("bin");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let launcher = shared_launcher();
        for tool in [
            "ffmpeg",
            "ffprobe",
            "upscaler",
            "realesrgan-ncnn-vulkan",
            "realesrgan-rocm",
        ] {
            std::os::unix::fs::symlink(launcher, bin.join(tool)).unwrap();
        }
        let input = dir.0.join("Tape One.mkv");
        fs::write(&input, b"source").unwrap();
        fs::create_dir_all(dir.0.join("models")).unwrap();
        let mut spec = JobSpec::defaults(
            variant,
            &input,
            &dir.0.join("out/Tape One.upscale.mkv"),
            &dir.0.join("work"),
        );
        spec.models_dir = dir.0.join("models");
        spec.skip_sync_fix = true;
        fs::create_dir_all(dir.0.join("out")).unwrap();
        let kit = Self {
            bins: Binaries {
                ffmpeg: bin.join("ffmpeg"),
                ffprobe: bin.join("ffprobe"),
                upscaler: bin.join("upscaler"),
            },
            dir,
            state,
            spec,
        };
        kit.set(cfg);
        kit
    }

    pub fn set(&self, cfg: &StubCfg) {
        fs::write(self.state.join("config.json"), cfg.json()).unwrap();
    }

    /// Every recorded call as `(tool, arguments)`, oldest first.
    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        let text = fs::read_to_string(self.state.join("calls.txt")).unwrap_or_default();
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for line in text.lines() {
            if let Some(t) = line.strip_prefix("CALL ") {
                out.push((t.to_string(), Vec::new()));
            } else if let Some(last) = out.last_mut() {
                last.1.push(line.to_string());
            }
        }
        out
    }

    pub fn calls_of(&self, tool: &str) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter(|(t, _)| t == tool)
            .map(|(_, a)| a)
            .collect()
    }
}
