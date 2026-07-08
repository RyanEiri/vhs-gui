//! Native Rust catalog for the 5 VapourSynth deinterlace/telecine wrappers
//! (formerly `vhs_qtgmc_only.sh`, `vhs_ivtc.sh`, `vhs_ivtc_decombed.sh`,
//! `vhs_field_align.sh`, `vhs_vdecimate.sh`). Each spawns `vspipe -c y4m
//! <vpy> - | ffmpeg ...` as a native two-process pipe via
//! `PipelineJob::start_native_pipe` — the `.vpy` VapourSynth scripts and
//! ffmpeg/vspipe themselves are never reimplemented, only the bash
//! orchestration around them.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::pipeline::PipelineJob;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsOp {
    Qtgmc,
    Ivtc,
    IvtcDecombed,
    FieldAlign,
    Vdecimate,
}

impl VsOp {
    fn vpy_filename(self) -> &'static str {
        match self {
            VsOp::Qtgmc => "qtgmc.vpy",
            VsOp::Ivtc => "ivtc.vpy",
            VsOp::IvtcDecombed => "ivtc_decombed.vpy",
            VsOp::FieldAlign => "field_align.vpy",
            VsOp::Vdecimate => "vdecimate.vpy",
        }
    }

    fn output_suffix(self) -> &'static str {
        match self {
            VsOp::Qtgmc => "_QTGMC",
            VsOp::Ivtc => "_IVTC",
            VsOp::IvtcDecombed => "_IVTC_DECOMBED",
            VsOp::FieldAlign => "_ALIGNED",
            VsOp::Vdecimate => "_VD",
        }
    }

    pub fn label_verb(self) -> &'static str {
        match self {
            VsOp::Qtgmc => "QTGMC",
            VsOp::Ivtc => "IVTC",
            VsOp::IvtcDecombed => "IVTC+Decomb",
            VsOp::FieldAlign => "Field Align",
            VsOp::Vdecimate => "VDecimate",
        }
    }

    /// Per-op `VS_*` env defaults, matching the bash scripts' `${VAR:-default}`.
    /// `VS_INPUT` is set separately by the caller (it's the input path).
    fn vs_env_defaults(self) -> Vec<(&'static str, String)> {
        match self {
            VsOp::Qtgmc => vec![
                ("VS_TFF", "1".into()),
                ("VS_FPSDIV", "2".into()),
                ("VS_PRESET", "Slower".into()),
            ],
            VsOp::Ivtc => vec![("VS_TFF", "1".into())],
            VsOp::IvtcDecombed => vec![("VS_TFF", "1".into()), ("VS_DECOMB_PRESET", "Fast".into())],
            VsOp::FieldAlign => vec![
                ("VS_TFF", "1".into()),
                ("VS_FIELD_SHIFT", "1.0".into()),
                ("VS_SHIFT_FIELD", "bottom".into()),
            ],
            VsOp::Vdecimate => vec![],
        }
    }

    fn needs_audio_probe(self) -> bool {
        matches!(self, VsOp::Vdecimate)
    }
}

/// Mirrors the bash `${IN%.mkv}<suffix>.mkv` derivation.
pub fn output_path(op: VsOp, input: &Path) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!("{stem}{}.mkv", op.output_suffix()))
}

/// Everything resolved during preflight, ready for the command builders.
struct Resolved {
    vspipe: PathBuf,
    ffmpeg: PathBuf,
    output: PathBuf,
    has_audio: bool,
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    let home = std::env::var("HOME").unwrap_or_default();
    let home_bin = format!("{home}/bin");
    std::iter::once(home_bin.as_str())
        .chain(path_var.split(':'))
        .map(|dir| Path::new(dir).join(name))
        .find(|p| p.is_file())
}

/// Validates inputs/tools exist and are runnable before anything is spawned —
/// mirrors the bash scripts' `[[ -f ]]`/`[[ -x ]]` checks with the same error
/// text, but fails fast with no half-started pipe.
fn preflight(op: VsOp, input: &Path, cfg: &Config) -> anyhow::Result<Resolved> {
    if !input.is_file() {
        anyhow::bail!("input not found: {}", input.display());
    }

    let vpy = cfg.tools_dir().join(op.vpy_filename());
    if !vpy.is_file() {
        anyhow::bail!("{} not found: {}", op.vpy_filename(), vpy.display());
    }

    let vspipe =
        find_in_path("vspipe").ok_or_else(|| anyhow::anyhow!("vspipe not found in PATH"))?;
    let ffmpeg = PathBuf::from("/usr/bin/ffmpeg");
    if !ffmpeg.is_file() {
        anyhow::bail!("ffmpeg not executable: {}", ffmpeg.display());
    }

    let has_audio = if op.needs_audio_probe() {
        probe_has_audio(input)
    } else {
        true
    };

    Ok(Resolved {
        vspipe,
        ffmpeg,
        output: output_path(op, input),
        has_audio,
    })
}

/// `ffprobe -select_streams a:0 -show_entries stream=index` — non-empty
/// stdout means an audio stream is present. Mirrors `vhs_vdecimate.sh`.
fn probe_has_audio(input: &Path) -> bool {
    let out = Command::new("/usr/bin/ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output();
    match out {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
        Err(_) => false,
    }
}

fn vpy_path(cfg: &Config, op: VsOp) -> PathBuf {
    cfg.tools_dir().join(op.vpy_filename())
}

/// Build the `vspipe` producer Command (env + args). Stdio/process-group are
/// wired by `PipelineJob::start_native_pipe`.
fn build_vspipe(op: VsOp, cfg: &Config, input: &Path, r: &Resolved) -> Command {
    let mut cmd = Command::new(&r.vspipe);
    cmd.args(["-c", "y4m"]).arg(vpy_path(cfg, op)).arg("-");
    cmd.env("VS_INPUT", input);
    for (k, v) in op.vs_env_defaults() {
        cmd.env(k, v);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let vsrepo_py = format!("{home}/.local/share/vsrepo/py");
    let existing = std::env::var("PYTHONPATH").unwrap_or_default();
    let pythonpath = if existing.is_empty() {
        vsrepo_py
    } else {
        format!("{vsrepo_py}:{existing}")
    };
    cmd.env("PYTHONPATH", pythonpath);
    cmd
}

/// Build the `ffmpeg` consumer Command (args only). Stdin (piped from
/// vspipe) and stdout/stderr/process-group are wired by
/// `PipelineJob::start_native_pipe`.
fn build_ffmpeg(input: &Path, r: &Resolved) -> Command {
    let mut cmd = Command::new(&r.ffmpeg);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-y",
        "-thread_queue_size",
        "1024",
        "-f",
        "yuv4mpegpipe",
        "-i",
        "-",
    ]);
    if r.has_audio {
        cmd.args(["-thread_queue_size", "1024", "-i"]).arg(input);
        cmd.args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "yuv422p",
            "-c:a",
            "copy",
            "-shortest",
        ]);
    } else {
        cmd.args(["-c:v", "ffv1", "-level", "3", "-pix_fmt", "yuv422p"]);
    }
    cmd.arg(&r.output);
    cmd
}

/// Preflight, build both commands, and spawn the native pipe job.
pub fn launch(op: VsOp, input: &Path, cfg: &Config, label: String) -> anyhow::Result<PipelineJob> {
    let resolved = preflight(op, input, cfg)?;
    let producer = build_vspipe(op, cfg, input, &resolved);
    let consumer = build_ffmpeg(input, &resolved);
    PipelineJob::start_native_pipe(label, input, producer, consumer, &cfg.log_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_path_suffixes() {
        let input = Path::new("/videos/stabilized/seg001_STABLE.mkv");
        assert_eq!(
            output_path(VsOp::Qtgmc, input),
            PathBuf::from("/videos/stabilized/seg001_STABLE_QTGMC.mkv")
        );
        assert_eq!(
            output_path(VsOp::Ivtc, input),
            PathBuf::from("/videos/stabilized/seg001_STABLE_IVTC.mkv")
        );
        assert_eq!(
            output_path(VsOp::IvtcDecombed, input),
            PathBuf::from("/videos/stabilized/seg001_STABLE_IVTC_DECOMBED.mkv")
        );
        assert_eq!(
            output_path(VsOp::FieldAlign, input),
            PathBuf::from("/videos/stabilized/seg001_STABLE_ALIGNED.mkv")
        );
        assert_eq!(
            output_path(VsOp::Vdecimate, input),
            PathBuf::from("/videos/stabilized/seg001_STABLE_VD.mkv")
        );
    }

    #[test]
    fn vs_env_defaults_match_bash() {
        let qtgmc: Vec<_> = VsOp::Qtgmc.vs_env_defaults();
        assert_eq!(
            qtgmc,
            vec![
                ("VS_TFF", "1".to_string()),
                ("VS_FPSDIV", "2".to_string()),
                ("VS_PRESET", "Slower".to_string()),
            ]
        );
        assert_eq!(
            VsOp::Ivtc.vs_env_defaults(),
            vec![("VS_TFF", "1".to_string())]
        );
        assert_eq!(
            VsOp::IvtcDecombed.vs_env_defaults(),
            vec![
                ("VS_TFF", "1".to_string()),
                ("VS_DECOMB_PRESET", "Fast".to_string()),
            ]
        );
        assert_eq!(
            VsOp::FieldAlign.vs_env_defaults(),
            vec![
                ("VS_TFF", "1".to_string()),
                ("VS_FIELD_SHIFT", "1.0".to_string()),
                ("VS_SHIFT_FIELD", "bottom".to_string()),
            ]
        );
        assert_eq!(
            VsOp::Vdecimate.vs_env_defaults(),
            Vec::<(&str, String)>::new()
        );
    }

    #[test]
    fn only_vdecimate_needs_audio_probe() {
        assert!(VsOp::Vdecimate.needs_audio_probe());
        assert!(!VsOp::Qtgmc.needs_audio_probe());
        assert!(!VsOp::Ivtc.needs_audio_probe());
        assert!(!VsOp::IvtcDecombed.needs_audio_probe());
        assert!(!VsOp::FieldAlign.needs_audio_probe());
    }

    /// End-to-end smoke test against real vspipe/ffmpeg/QTGMC on a short
    /// clip. Not run by default (`cargo test -- --ignored`) since it needs
    /// real sample media and takes real wall-clock time.
    #[test]
    #[ignore]
    fn qtgmc_end_to_end_smoke() {
        let cfg = Config::default();
        let input = PathBuf::from(
            "/tmp/claude-1000/-home-ryan-Videos/129570ca-1234-4fe4-8e4c-d071b8ca2f34/scratchpad/vstest/clip_STABLE.mkv",
        );
        assert!(input.is_file(), "test clip missing: {}", input.display());
        let out = output_path(VsOp::Qtgmc, &input);
        let _ = std::fs::remove_file(&out);

        let mut job =
            launch(VsOp::Qtgmc, &input, &cfg, "smoke-test QTGMC".into()).expect("launch failed");

        // Let it run briefly, then verify pause/resume via ps state.
        std::thread::sleep(std::time::Duration::from_secs(5));
        job.poll();
        println!(
            "after 5s: done={} current_frame={} current_time_secs={} pgid={}",
            job.done, job.current_frame, job.current_time_secs, job.pgid
        );
        let pgrep_before = std::process::Command::new("pgrep")
            .args(["-a", "-g", &job.pgid.to_string()])
            .output()
            .expect("pgrep failed");
        println!(
            "pgrep -a -g {}:\n{}",
            job.pgid,
            String::from_utf8_lossy(&pgrep_before.stdout)
        );
        assert!(!job.done, "job finished implausibly fast");

        job.toggle_pause();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let pgid = job.pgid;
        let pgrep_pids = std::process::Command::new("pgrep")
            .args(["-g", &pgid.to_string()])
            .output()
            .expect("pgrep failed");
        let pids: Vec<String> = String::from_utf8_lossy(&pgrep_pids.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(
            !pids.is_empty(),
            "no processes found in group {pgid} while paused"
        );
        // Read state (3rd field) directly from /proc/<pid>/stat — avoids ps's
        // ambiguous -g semantics (real gid vs pgid) across ps builds.
        let states: Vec<String> = pids
            .iter()
            .map(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .and_then(|s| s.rsplit(')').next().map(|s| s.to_string()))
                    .and_then(|rest| rest.split_whitespace().next().map(|s| s.to_string()))
                    .unwrap_or_default()
            })
            .collect();
        println!("process states while paused (pids {pids:?}): {states:?}");
        assert!(
            states.iter().any(|s| s == "T"),
            "expected at least one stopped (T) process while paused, got: {states:?}"
        );

        job.toggle_pause(); // resume
        std::thread::sleep(std::time::Duration::from_millis(300));

        job.cancel();
        for _ in 0..50 {
            job.poll();
            if job.done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(job.done, "job did not finish after cancel");

        // No orphaned vspipe/ffmpeg left behind.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let leftover = std::process::Command::new("pgrep")
            .args(["-g", &pgid.to_string()])
            .output()
            .expect("pgrep failed");
        assert!(
            leftover.stdout.is_empty(),
            "leftover processes in group {pgid}: {}",
            String::from_utf8_lossy(&leftover.stdout)
        );
    }
}
