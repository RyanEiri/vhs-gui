//! Native Rust port of `vhs_audio_cleanup.sh`: a heavier, manual-only audio
//! pass for tapes whose light `denoise.sh` pass didn't fully clean up.
//! Notches mains hum (fundamental + 2nd/3rd harmonics) via SoX `bandreject`,
//! then applies SoX `noiseprof`/`noisered` for broadband noise, matching the
//! bash script's stage order and defaults exactly so the two stay in
//! lockstep. Video is always copied bit-exact; audio stays PCM.
//!
//! Unlike `fix_sync.rs` (one ffmpeg call), this is a *sequence* of
//! independent processes — see `PipelineJob::start_native_sequence`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use crate::config::Config;
use crate::pipeline::{PipelineJob, SeqCtx, SeqStep};

const FFMPEG: &str = "/usr/bin/ffmpeg";
const SOX: &str = "/usr/bin/sox";

const HUM_HZ: u32 = 60;

// loudnorm true-peak ceiling and loudness-range target: fixed, not exposed
// as knobs (TP=-1.5 leaves headroom for lossy re-encodes downstream; LRA=11
// is ffmpeg's own documented default). Only the integrated-loudness target
// is user-facing (`Params::loudnorm_target_i`) since that's the one that
// actually controls "how loud."
const LOUDNORM_TP: &str = "-1.5";
const LOUDNORM_LRA: &str = "11";

/// Tunable knobs, surfaced in the GUI rather than hardcoded (see
/// `panels/upscale.rs`'s Audio Cleanup inline panel). The *fixed* default
/// noise-sample window (0:00, 1.0s) turned out to be a real problem on tapes
/// with no quiet lead-in: if program audio starts immediately, `noiseprof`
/// captures dialogue/music as "noise" and `noisered` then suppresses similar
/// content throughout the whole file, producing muffled/artifacty audio
/// rather than actually cleaning anything up. `noise_start_secs` lets the
/// user point the sample at an actual quiet spot on that specific tape.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub hum_enable: bool,
    pub noise_start_secs: f64,
    pub noise_len_secs: f64,
    pub nr_amount: f64,
    /// Integrated loudness target in LUFS for the final normalization pass
    /// (ffmpeg `loudnorm`, two-pass). Replaces the old fixed `sox norm -1`
    /// (peak-only: boosts the whole file by one gain factor so the single
    /// loudest moment hits -1dB, which leaves quiet passages quiet relative
    /// to it — measured -37dB to -15dB across one real clip after norm).
    /// loudnorm instead targets consistent *perceived* loudness throughout.
    /// -16 LUFS is a standard target for spoken-word/podcast content; music
    /// or a tape with wider dynamic range may want it lower (e.g. -20).
    /// Confirmed on a real 3-hour spoken-word capture with an unusually wide
    /// 17.7 LU original range (`EDIT_MASTER-MESSAGE_FROM_NAM.mkv`): -16
    /// sounded "too hot," -20 was preferred. Kept as the per-tape knob it
    /// already was rather than becoming the new default -- this is one
    /// data point, not a reason to assume -20 suits every tape.
    pub loudnorm_target_i: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            hum_enable: true,
            noise_start_secs: 0.0,
            noise_len_secs: 1.0,
            // Bash script default; found to sound harsh/gate-y on complex
            // program material (voice+music) when the noise profile is even
            // slightly contaminated by real content -- lowered here so the
            // GUI's out-of-the-box result errs gentle. Raise it once the
            // noise-sample window is confirmed to be genuinely quiet.
            nr_amount: 0.15,
            loudnorm_target_i: -16.0,
        }
    }
}

/// Pull `"key" : "value"` out of ffmpeg loudnorm's `print_format=json`
/// summary (written to stderr). Hand-rolled rather than pulling in a JSON
/// dependency for one fixed, well-known output shape — same spirit as this
/// codebase's other small manual parsers (`pipeline.rs`'s `parse_field`/
/// `parse_hms`, `fix_sync.rs`'s ffprobe field parsing).
fn extract_json_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let idx = text.find(&needle)?;
    let rest = &text[idx + needle.len()..];
    let rest = &rest[rest.find(':')? + 1..];
    let start = rest.find('"')? + 1;
    let rest = &rest[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn hhmmss(secs: f64) -> String {
    let secs = secs.max(0.0);
    let h = (secs / 3600.0) as u64;
    let m = ((secs % 3600.0) / 60.0) as u64;
    let s = secs % 60.0;
    format!("{h:02}:{m:02}:{s:05.2}")
}

fn af_chain() -> String {
    "highpass=f=20,aresample=async=0:first_pts=0,asetpts=N/SR/TB".to_string()
}

fn threads() -> String {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .to_string()
}

/// Compute the cleaned-audio output path for `input`, keeping
/// `library.rs`'s filename-based classification intact.
///
/// `scan_stabilized_dir` buckets purely on filename: `ends_with("_VD.mkv")`
/// -> Edit Master (VD), else `starts_with("EDIT_MASTER")` -> Edit Master. A
/// naive `{stem}_ACLEAN.mkv` on `EDIT_MASTER-Foo_VD.mkv` would produce
/// `EDIT_MASTER-Foo_VD_ACLEAN.mkv`, which no longer ends with `_VD.mkv` and
/// would silently reappear under Edit Master (offering VDecimate again on
/// already-VDecimated content) instead of Edit Master (VD). Stripping the
/// `_VD` suffix first and re-appending it after `_ACLEAN` keeps the ordering
/// `library.rs` expects.
pub fn output_path(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let name = input.file_name().and_then(|s| s.to_str()).unwrap_or("out");
    let stem = name.strip_suffix(".mkv").unwrap_or(name);
    let cleaned = if let Some(base) = stem.strip_suffix("_VD") {
        format!("{base}_ACLEAN_VD.mkv")
    } else {
        format!("{stem}_ACLEAN.mkv")
    };
    dir.join(cleaned)
}

fn unique_work_dir() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("vhs-gui-audiocleanup-{}-{ts}", std::process::id()))
}

/// Build the stage pipeline and hand it to `PipelineJob::start_native_sequence`.
pub fn launch(
    input: &Path,
    output: &Path,
    cfg: &Config,
    label: String,
    params: &Params,
) -> anyhow::Result<PipelineJob> {
    if !Path::new(FFMPEG).is_file() {
        anyhow::bail!("ffmpeg not found: {FFMPEG}");
    }
    if !Path::new(SOX).is_file() {
        anyhow::bail!("sox not found: {SOX}. Install with: sudo apt-get install sox");
    }
    if !input.is_file() {
        anyhow::bail!("input not found: {}", input.display());
    }
    if let Some(dir) = output.parent() {
        fs::create_dir_all(dir)?;
    }

    let work_dir = unique_work_dir();
    fs::create_dir_all(&work_dir)?;

    let full_wav = work_dir.join("full.wav");
    let hum_wav = work_dir.join("hum.wav");
    let sample_wav = work_dir.join("sample.wav");
    let noise_prof = work_dir.join("noise.prof");
    let clean_wav = work_dir.join("clean.wav");
    let norm_wav = work_dir.join("norm.wav");

    let af = af_chain();
    let th = threads();

    let mut extract_full = Command::new(FFMPEG);
    extract_full
        .args(["-hide_banner", "-nostdin", "-y", "-fflags", "+genpts", "-i"])
        .arg(input)
        .args(["-vn", "-map", "0:a:0", "-af"])
        .arg(&af)
        .args([
            "-ac",
            "2",
            "-ar",
            "48000",
            "-c:a",
            "pcm_s16le",
            "-threads",
            &th,
        ])
        .arg(&full_wav);

    let noise_ss = hhmmss(params.noise_start_secs);
    let noise_t = hhmmss(params.noise_len_secs);
    let nr_amount = params.nr_amount.to_string();

    let mut extract_sample = Command::new(FFMPEG);
    extract_sample
        .args(["-hide_banner", "-nostdin", "-y", "-fflags", "+genpts"])
        .args(["-ss", &noise_ss, "-t", &noise_t, "-i"])
        .arg(input)
        .args(["-vn", "-map", "0:a:0", "-af"])
        .arg(&af)
        .args([
            "-ac",
            "2",
            "-ar",
            "48000",
            "-c:a",
            "pcm_s16le",
            "-threads",
            &th,
        ])
        .arg(&sample_wav);

    let mut noiseprof = Command::new(SOX);
    noiseprof
        .arg(&sample_wav)
        .args(["-n", "noiseprof"])
        .arg(&noise_prof);

    let mut remux = Command::new(FFMPEG);
    remux
        .args(["-hide_banner", "-nostdin", "-y", "-fflags", "+genpts", "-i"])
        .arg(input)
        .args(["-fflags", "+genpts", "-i"])
        .arg(&norm_wav)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "pcm_s16le",
        ])
        .args(["-avoid_negative_ts", "make_zero", "-shortest"])
        .arg(output);

    // The hum-notch stage is skipped entirely (not just a no-op) when
    // disabled, so noisered runs against `full_wav` directly and no
    // notch-specific sox invocation exists to report/log.
    let mut steps: Vec<(&'static str, SeqStep)> =
        vec![("Extracting audio...", SeqStep::Cmd(extract_full))];
    let noisered_input = if params.hum_enable {
        let mut hum_notch = Command::new(SOX);
        hum_notch.arg(&full_wav).arg(&hum_wav).args([
            "bandreject",
            &HUM_HZ.to_string(),
            "2q",
            "bandreject",
            &(HUM_HZ * 2).to_string(),
            "2q",
            "bandreject",
            &(HUM_HZ * 3).to_string(),
            "2q",
        ]);
        steps.push(("Notching hum...", SeqStep::Cmd(hum_notch)));
        hum_wav
    } else {
        full_wav.clone()
    };

    let mut noisered = Command::new(SOX);
    noisered
        .arg(&noisered_input)
        .arg(&clean_wav)
        .arg("noisered")
        .arg(&noise_prof)
        .arg(&nr_amount);

    steps.push(("Sampling noise profile...", SeqStep::Cmd(extract_sample)));
    steps.push(("Building noise profile...", SeqStep::Cmd(noiseprof)));
    steps.push(("Reducing broadband noise...", SeqStep::Cmd(noisered)));
    steps.push((
        "Loudness normalizing...",
        SeqStep::Fn(loudnorm_step(
            clean_wav,
            norm_wav,
            params.loudnorm_target_i,
            th,
        )),
    ));
    steps.push(("Remuxing...", SeqStep::Cmd(remux)));

    PipelineJob::start_native_sequence(label, input, steps, Some(work_dir), &cfg.log_dir())
}

/// Build the two-pass `loudnorm` closure step: pass 1 measures `input`'s
/// actual loudness (ffmpeg `loudnorm ... print_format=json`, parsed off
/// stderr), pass 2 applies the correction using those measured values
/// (`linear=true` — a single fixed gain, not per-frame dynamic processing,
/// when the measured input is close enough to the target to normalize
/// linearly; ffmpeg falls back to its own dynamic algorithm otherwise).
/// Replaces the old fixed `sox norm -1`, which only looked at the single
/// loudest peak in the whole file (see `Params::loudnorm_target_i`'s doc
/// comment for why that under-served quiet passages).
fn loudnorm_step(
    input: PathBuf,
    output: PathBuf,
    target_i: f64,
    threads: String,
) -> Box<dyn FnOnce(&SeqCtx) -> bool + Send> {
    Box::new(move |ctx: &SeqCtx| {
        let target_i = target_i.to_string();

        let mut measure = Command::new(FFMPEG);
        measure
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(&input)
            .arg("-af")
            .arg(format!(
                "loudnorm=I={target_i}:TP={LOUDNORM_TP}:LRA={LOUDNORM_LRA}:print_format=json"
            ))
            .args(["-f", "null", "-"]);
        let (measured_ok, text) = ctx.run_captured(measure);
        if !measured_ok || ctx.cancelled() {
            return false;
        }
        let (Some(m_i), Some(m_tp), Some(m_lra), Some(m_thresh), Some(offset)) = (
            extract_json_field(&text, "input_i"),
            extract_json_field(&text, "input_tp"),
            extract_json_field(&text, "input_lra"),
            extract_json_field(&text, "input_thresh"),
            extract_json_field(&text, "target_offset"),
        ) else {
            return false;
        };

        let mut apply = Command::new(FFMPEG);
        apply
            .args(["-hide_banner", "-nostdin", "-y", "-i"])
            .arg(&input)
            .arg("-af")
            .arg(format!(
                "loudnorm=I={target_i}:TP={LOUDNORM_TP}:LRA={LOUDNORM_LRA}:\
                 measured_I={m_i}:measured_TP={m_tp}:measured_LRA={m_lra}:\
                 measured_thresh={m_thresh}:offset={offset}:linear=true:print_format=summary"
            ))
            .args(["-ar", "48000", "-c:a", "pcm_s16le", "-threads", &threads])
            .arg(&output);
        ctx.run(apply)
    })
}

// ---------------------------------------------------------------------------
// Quiet-spot scan — finds a genuinely silent stretch to use as the noise-
// sample window, instead of the user (or me, by hand from the terminal)
// scanning timestamps with ffmpeg astats. Not a PipelineJob: this doesn't
// produce an output file, it produces a suggested value to fill into the
// Audio Cleanup panel's fields. Same polled-background-thread idiom as
// pipeline.rs's SeqShared/SeqCtx (Arc<Mutex<..>> state, pid tracked for
// cancel), but self-contained here since it doesn't fit PipelineJob's shape.
// ---------------------------------------------------------------------------

const SILENCE_THRESHOLD_DB: &str = "-35dB";
const SILENCE_MIN_DUR: &str = "2";

pub struct QuietSpot {
    pub start_secs: f64,
    pub len_secs: f64,
    pub rms_db: f64,
    /// Full length of the silence gap this window was carved out of, for
    /// the UI's confirmation label (e.g. "6.2s quiet stretch").
    pub gap_secs: f64,
}

struct QuietSpotShared {
    current_pid: Mutex<Option<i32>>,
    cancel_requested: AtomicBool,
    /// `None` while running. `Ok(Some(_))` found a spot; `Ok(None)` the scan
    /// completed but nothing quiet enough turned up; `Err(_)` ffmpeg itself
    /// failed to run.
    result: Mutex<Option<Result<Option<QuietSpot>, String>>>,
}

pub struct QuietSpotScan {
    shared: Arc<QuietSpotShared>,
    started_at: Instant,
}

impl QuietSpotScan {
    pub fn start(input: &Path) -> Self {
        let shared = Arc::new(QuietSpotShared {
            current_pid: Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
            result: Mutex::new(None),
        });
        let thread_shared = Arc::clone(&shared);
        let input = input.to_owned();
        std::thread::spawn(move || {
            let outcome = run_scan(&input, &thread_shared);
            *thread_shared.result.lock().unwrap() = Some(outcome);
        });
        Self {
            shared,
            started_at: Instant::now(),
        }
    }

    /// Non-blocking poll. Returns `Some(_)` exactly once the scan finishes;
    /// keep calling `poll()` (it returns `None`) while it's still running.
    pub fn poll(&self) -> Option<Result<Option<QuietSpot>, String>> {
        self.shared.result.lock().unwrap().take()
    }

    pub fn cancel(&self) {
        self.shared.cancel_requested.store(true, Ordering::SeqCst);
        if let Some(pid) = *self.shared.current_pid.lock().unwrap() {
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;
            let _ = kill(Pid::from_raw(pid), Signal::SIGINT);
        }
    }

    pub fn elapsed_str(&self) -> String {
        let d = self.started_at.elapsed();
        let m = d.as_secs() / 60;
        let s = d.as_secs() % 60;
        format!("{m}:{s:02}")
    }
}

fn run_scan(input: &Path, shared: &QuietSpotShared) -> Result<Option<QuietSpot>, String> {
    if !Path::new(FFMPEG).is_file() {
        return Err(format!("ffmpeg not found: {FFMPEG}"));
    }
    if !input.is_file() {
        return Err(format!("input not found: {}", input.display()));
    }

    let mut cmd = Command::new(FFMPEG);
    cmd.args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args(["-vn", "-af"])
        .arg(format!(
            "silencedetect=n={SILENCE_THRESHOLD_DB}:d={SILENCE_MIN_DUR}"
        ))
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;
    *shared.current_pid.lock().unwrap() = Some(child.id() as i32);
    let output = child.wait_with_output();
    *shared.current_pid.lock().unwrap() = None;

    if shared.cancel_requested.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let output = output.map_err(|e| format!("ffmpeg failed: {e}"))?;
    let text = String::from_utf8_lossy(&output.stderr);

    let Some((gap_start, gap_end, gap_len)) = longest_silence_gap(&text) else {
        return Ok(None);
    };

    let (start_secs, len_secs) = pick_quiet_window(gap_start, gap_end);
    let rms_db = measure_rms(input, start_secs, len_secs).unwrap_or(f64::NAN);

    Ok(Some(QuietSpot {
        start_secs,
        len_secs,
        rms_db,
        gap_secs: gap_len,
    }))
}

/// Parse ffmpeg `silencedetect` stderr for `silence_start: X` /
/// `silence_end: Y | silence_duration: Z` pairs and return the widest gap
/// found, as `(start, end, duration)`. Hand-rolled line scanning, same
/// spirit as `extract_json_field` — one fixed, well-known output shape.
fn longest_silence_gap(text: &str) -> Option<(f64, f64, f64)> {
    let mut best: Option<(f64, f64, f64)> = None;
    let mut pending_start: Option<f64> = None;

    for line in text.lines() {
        if let Some(idx) = line.find("silence_start: ") {
            let val = &line[idx + "silence_start: ".len()..];
            pending_start = val.split_whitespace().next().and_then(|s| s.parse().ok());
        } else if let Some(idx) = line.find("silence_end: ") {
            let rest = &line[idx + "silence_end: ".len()..];
            let end: Option<f64> = rest.split_whitespace().next().and_then(|s| s.parse().ok());
            let dur = rest
                .find("silence_duration: ")
                .and_then(|d| rest[d + "silence_duration: ".len()..].trim().parse().ok());
            if let (Some(start), Some(end), Some(dur)) = (pending_start.take(), end, dur)
                && best.is_none_or(|(_, _, best_dur)| dur > best_dur)
            {
                best = Some((start, end, dur));
            }
        }
    }
    best
}

/// Pad 0.5s off each end of the gap (avoid a click/fade right at the
/// silence boundary) and clamp the resulting window to 1.0..=2.0s.
fn pick_quiet_window(gap_start: f64, gap_end: f64) -> (f64, f64) {
    const PAD: f64 = 0.5;
    let inner_start = gap_start + PAD;
    let inner_len = (gap_end - gap_start - 2.0 * PAD).clamp(1.0, 2.0);
    (inner_start, inner_len)
}

fn measure_rms(input: &Path, start_secs: f64, len_secs: f64) -> Option<f64> {
    let out = Command::new(FFMPEG)
        .args(["-hide_banner", "-nostdin", "-loglevel", "info"])
        .args(["-ss", &hhmmss(start_secs), "-t", &hhmmss(len_secs), "-i"])
        .arg(input)
        .args(["-vn", "-af", "astats", "-f", "null", "-"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .find(|l| l.contains("RMS level dB"))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_path_plain_edit_master() {
        let p = output_path(Path::new("/x/captures/stabilized/EDIT_MASTER-Foo.mkv"));
        assert_eq!(
            p,
            PathBuf::from("/x/captures/stabilized/EDIT_MASTER-Foo_ACLEAN.mkv")
        );
    }

    #[test]
    fn output_path_vd_edit_master_keeps_vd_suffix_last() {
        let p = output_path(Path::new("/x/captures/stabilized/EDIT_MASTER-Foo_VD.mkv"));
        assert_eq!(
            p,
            PathBuf::from("/x/captures/stabilized/EDIT_MASTER-Foo_ACLEAN_VD.mkv")
        );
        // Still ends with _VD.mkv, so library.rs's classification still
        // buckets this under Edit Master (VD), not Edit Master.
        assert!(p.to_string_lossy().ends_with("_VD.mkv"));
    }

    #[test]
    fn output_path_viewer_file() {
        let p = output_path(Path::new("/x/captures/viewer/VHS Trailer.mkv"));
        assert_eq!(
            p,
            PathBuf::from("/x/captures/viewer/VHS Trailer_ACLEAN.mkv")
        );
    }

    #[test]
    fn hum_harmonics_match_bash_defaults() {
        // vhs_audio_cleanup.sh: bandreject $HUM_HZ 2q, $((HUM_HZ*2)) 2q, $((HUM_HZ*3)) 2q
        assert_eq!(HUM_HZ, 60);
        assert_eq!(HUM_HZ * 2, 120);
        assert_eq!(HUM_HZ * 3, 180);
    }

    #[test]
    fn defaults_match_bash_script_except_gentler_nr_amount() {
        // The noise-sample window shape still matches vhs_audio_cleanup.sh
        // exactly. NR_AMOUNT is intentionally lower than the bash script's
        // 0.25 -- see Params::default's doc comment. Final normalization no
        // longer matches the bash script at all: it uses fixed `sox norm
        // -1` (peak-only), the GUI now uses two-pass loudnorm targeting
        // perceived loudness (see loudnorm_step's doc comment for why).
        assert_eq!(Params::default().nr_amount, 0.15);
        assert_eq!(Params::default().noise_start_secs, 0.0);
        assert_eq!(Params::default().noise_len_secs, 1.0);
        assert_eq!(Params::default().loudnorm_target_i, -16.0);
        assert!(Params::default().hum_enable);
    }

    #[test]
    fn extract_json_field_parses_loudnorm_summary() {
        let text = r#"[Parsed_loudnorm_0 @ 0x1]

{
	"input_i" : "-23.71",
	"input_tp" : "-4.35",
	"input_lra" : "6.00",
	"input_thresh" : "-34.02",
	"output_i" : "-16.01",
	"output_tp" : "-1.50",
	"output_lra" : "6.10",
	"output_thresh" : "-26.34",
	"normalization_type" : "dynamic",
	"target_offset" : "0.01"
}
"#;
        assert_eq!(
            extract_json_field(text, "input_i").as_deref(),
            Some("-23.71")
        );
        assert_eq!(
            extract_json_field(text, "input_tp").as_deref(),
            Some("-4.35")
        );
        assert_eq!(
            extract_json_field(text, "input_lra").as_deref(),
            Some("6.00")
        );
        assert_eq!(
            extract_json_field(text, "input_thresh").as_deref(),
            Some("-34.02")
        );
        assert_eq!(
            extract_json_field(text, "target_offset").as_deref(),
            Some("0.01")
        );
        assert_eq!(extract_json_field(text, "missing_key"), None);
    }

    #[test]
    fn hhmmss_formats_for_ffmpeg_ss() {
        assert_eq!(hhmmss(0.0), "00:00:00.00");
        assert_eq!(hhmmss(1.0), "00:00:01.00");
        assert_eq!(hhmmss(90.5), "00:01:30.50");
        assert_eq!(hhmmss(3661.25), "01:01:01.25");
    }

    #[test]
    fn pick_quiet_window_pads_and_clamps() {
        // 8s gap: 0.5s padding each end -> 7s inner, clamped down to 2.0s max.
        assert_eq!(pick_quiet_window(296.0, 304.0), (296.5, 2.0));
        // 3s gap: padding leaves 2s inner -> within range, used as-is.
        assert_eq!(pick_quiet_window(100.0, 103.0), (100.5, 2.0));
        // 1.5s gap: padding would leave 0.5s inner -> clamped up to the 1.0s floor.
        assert_eq!(pick_quiet_window(50.0, 51.5), (50.5, 1.0));
    }

    #[test]
    fn longest_silence_gap_picks_widest_of_several() {
        // Real silencedetect output shape, captured from this session's
        // manual analysis of EDIT_MASTER-MESSAGE_FROM_NAM.mkv.
        let text = "\
[silencedetect @ 0x1] silence_start: 125.413
[silencedetect @ 0x1] silence_end: 127.973 | silence_duration: 2.55983
[silencedetect @ 0x1] silence_start: 295.349
[silencedetect @ 0x1] silence_end: 303.197 | silence_duration: 7.84769
[silencedetect @ 0x1] silence_start: 335.639
[silencedetect @ 0x1] silence_end: 341.351 | silence_duration: 5.71231
";
        let (start, end, dur) = longest_silence_gap(text).expect("a gap");
        assert_eq!(start, 295.349);
        assert_eq!(end, 303.197);
        assert!((dur - 7.84769).abs() < 1e-6);
    }

    #[test]
    fn longest_silence_gap_none_when_no_matches() {
        assert_eq!(longest_silence_gap("nothing relevant here\n"), None);
    }

    #[test]
    fn longest_silence_gap_ignores_unpaired_start() {
        // A silence_start with no matching silence_end (e.g. truncated
        // output, or silence running to EOF) must not be picked.
        let text = "\
[silencedetect @ 0x1] silence_start: 10.0
[silencedetect @ 0x1] silence_end: 12.0 | silence_duration: 2.0
[silencedetect @ 0x1] silence_start: 999.0
";
        let (start, end, dur) = longest_silence_gap(text).expect("a gap");
        assert_eq!((start, end), (10.0, 12.0));
        assert!((dur - 2.0).abs() < 1e-6);
    }

    /// End-to-end smoke test against a synthetic input with a delayed 60Hz
    /// hum tone, mirroring `vhs-cli/test_audio_cleanup.sh`'s proven
    /// methodology exactly: the hum starts *after* the 1s noise-sample
    /// window, so noisered's own profile can't have captured it and the
    /// dedicated notch stage is the only thing that can remove it.
    /// Not run by default (`cargo test -- --ignored`) — spawns real
    /// ffmpeg/sox processes.
    #[test]
    #[ignore]
    fn audio_cleanup_end_to_end_smoke() {
        let cfg = Config::default();
        let dir = std::env::temp_dir().join("vhs_gui_audio_cleanup_smoke");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Realistic structure: 1s of noise-only lead-in (what NOISE_SS/
        // NOISE_T=1.0s assumes -- blank tape/room tone before program
        // audio starts), then 2s of tone+hum+noise. Getting this wrong
        // matters a lot here: an earlier version of this test had the tone
        // span the *whole* clip, so it bled into the noise-sample window --
        // noiseprof then treated the tone itself as "noise" and noisered
        // nuked it, which (combined with sox `norm`'s peak-based gain
        // amplifying whatever was left, hum-notch leakage included) made a
        // naive absolute-dB before/after comparison actively backwards.
        // Program audio is also mixed without amix's automatic
        // normalization (`normalize=0`) and the noise floor kept well below
        // the tone, so the tone survives cleanup as it would on a real tape.
        let lead = dir.join("lead.wav");
        assert!(
            Command::new(FFMPEG)
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
                .args(["-f", "lavfi", "-i", "anoisesrc=duration=1:amplitude=0.02"])
                .args(["-c:a", "pcm_s16le"])
                .arg(&lead)
                .status()
                .expect("failed to build lead-in")
                .success()
        );

        let prog = dir.join("prog.wav");
        assert!(
            Command::new(FFMPEG)
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
                .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
                .args(["-f", "lavfi", "-i", "sine=frequency=60:duration=2"])
                .args(["-f", "lavfi", "-i", "anoisesrc=duration=2:amplitude=0.02"])
                .args([
                    "-filter_complex",
                    "[1:a]volume=0.3[hum];[0:a][hum][2:a]amix=inputs=3:duration=first:dropout_transition=0:normalize=0[aout]",
                ])
                .args(["-map", "[aout]", "-c:a", "pcm_s16le"])
                .arg(&prog)
                .status()
                .expect("failed to build program audio")
                .success()
        );

        let audio = dir.join("audio.wav");
        assert!(
            Command::new(FFMPEG)
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"])
                .arg(&lead)
                .arg("-i")
                .arg(&prog)
                .args(["-filter_complex", "[0:a][1:a]concat=n=2:v=0:a=1[aout]"])
                .args(["-map", "[aout]", "-c:a", "pcm_s16le"])
                .arg(&audio)
                .status()
                .expect("failed to concat audio")
                .success()
        );

        let noisy = dir.join("noisy.mkv");
        assert!(
            Command::new(FFMPEG)
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
                .args(["-f", "lavfi", "-i", "color=c=black:s=64x64:d=3", "-i"])
                .arg(&audio)
                .args(["-map", "0:v", "-map", "1:a"])
                .args([
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "pcm_s16le",
                    "-t",
                    "3"
                ])
                .arg(&noisy)
                .status()
                .expect("failed to mux synthetic input")
                .success()
        );

        let output = dir.join("clean.mkv");
        let mut job = launch(
            &noisy,
            &output,
            &cfg,
            "smoke-test audio_cleanup".into(),
            &Params::default(),
        )
        .expect("launch failed");

        for _ in 0..200 {
            job.poll();
            if job.done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(job.done, "job did not finish in time");
        assert_eq!(job.exit_ok, Some(true));
        assert!(output.is_file());

        // Measure the 60Hz-band level *relative to* the overall level,
        // rather than an absolute dB, since the final loudnorm stage
        // (always applied) rescales the whole track towards a target
        // loudness -- an absolute before/after comparison would be swamped
        // by whatever gain that applies. The ratio cancels a uniform gain,
        // so it isolates the notch/noisered stages' actual spectral effect.
        // Verified against a hand-run copy of this exact pipeline with the
        // notch stage skipped: noisered alone moves this ratio by ~0dB on
        // a persistent tone, while the full pipeline (notch included) moves
        // it by >12dB -- so an 8dB bar cleanly discriminates "the notch
        // stage genuinely ran" from noisered's incidental effect.
        let db_level = |path: &Path, af: &str| -> f64 {
            let out = Command::new(FFMPEG)
                .args(["-hide_banner", "-nostdin", "-loglevel", "info"])
                .args(["-ss", "1", "-t", "2", "-i"])
                .arg(path)
                .args(["-af", af, "-f", "null", "-"])
                .output()
                .expect("ffmpeg astats failed");
            let text = String::from_utf8_lossy(&out.stderr);
            text.lines()
                .find(|l| l.contains("RMS level dB"))
                .and_then(|l| l.rsplit(' ').next())
                .and_then(|v| v.parse::<f64>().ok())
                .expect("RMS level not found in astats output")
        };
        let hum_vs_overall = |path: &Path| -> f64 {
            db_level(path, "bandpass=f=60:width_type=q:w=2,astats") - db_level(path, "astats")
        };

        let before = hum_vs_overall(&noisy);
        let after = hum_vs_overall(&output);
        assert!(
            after < before - 8.0,
            "60Hz hum not measurably reduced relative to overall level: before={before} after={after}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Builds a synthetic clip with a known silent gap and confirms
    /// `QuietSpotScan` finds it. Not run by default -- spawns real ffmpeg.
    #[test]
    #[ignore]
    fn quiet_spot_scan_finds_known_gap() {
        let dir = std::env::temp_dir().join("vhs_gui_quiet_spot_smoke");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 2s tone, 6s silence, 2s tone -- the silent gap is unambiguous.
        let input = dir.join("gapped.mkv");
        let status = Command::new(FFMPEG)
            .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
            .args(["-f", "lavfi", "-i", "anullsrc=duration=6"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
            .args(["-filter_complex", "[0:a][1:a][2:a]concat=n=3:v=0:a=1[aout]"])
            .args(["-map", "[aout]", "-c:a", "pcm_s16le"])
            .arg(&input)
            .status()
            .expect("failed to build synthetic input");
        assert!(status.success());

        let scan = QuietSpotScan::start(&input);
        let result = loop {
            if let Some(r) = scan.poll() {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };

        let spot = result
            .expect("scan should not error")
            .expect("should find the gap");
        // Gap runs 2.0..8.0 -- window should sit inside that, well clear of
        // the tone on either side.
        assert!(spot.start_secs >= 2.0 && spot.start_secs <= 8.0);
        assert!(spot.start_secs + spot.len_secs <= 8.5);
        assert!(spot.gap_secs > 5.0);

        let _ = fs::remove_dir_all(&dir);
    }
}
