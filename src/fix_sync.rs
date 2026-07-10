//! Native Rust port of `vhs_fix_sync.sh`: corrects A/V drift by computing an
//! `atempo` factor from independently-probed video/audio stream durations,
//! then re-encodes audio only (video is stream-copied). No pipe — sequential
//! ffprobe (4-level fallback) → arithmetic → one `ffmpeg` call.

use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::pipeline::PipelineJob;

const FFPROBE: &str = "/usr/bin/ffprobe";
const FFMPEG: &str = "/usr/bin/ffmpeg";

/// Below this drift percentage, correction isn't worth it (matches the bash
/// script's threshold exactly).
const DRIFT_THRESHOLD_PCT: f64 = 0.001;

fn ffprobe_field(input: &Path, selector: &str, entry: &str) -> Option<String> {
    let mut args = vec!["-v", "error"];
    if !selector.is_empty() {
        args.extend(["-select_streams", selector]);
    }
    args.extend(["-show_entries", entry, "-of", "default=nk=1:nw=1"]);
    let out = Command::new(FFPROBE).args(&args).arg(input).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn parse_hms(s: &str) -> Option<f64> {
    // "HH:MM:SS.mmm" (or more decimals). "N/A" is not numeric -> None.
    let mut parts = s.splitn(3, ':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn parse_time_base(tb: &str) -> Option<(f64, f64)> {
    let mut parts = tb.splitn(2, '/');
    let num: f64 = parts.next()?.parse().ok()?;
    let den: f64 = parts.next()?.parse().ok()?;
    if den == 0.0 { None } else { Some((num, den)) }
}

/// 4-fallback duration probe for one stream selector ("v:0" | "a:0"),
/// mirroring `get_stream_duration_seconds()` in vhs_fix_sync.sh exactly:
/// (1) stream.duration, (2) stream_tags:DURATION (HH:MM:SS.mmm),
/// (3) duration_ts * time_base, (4) format.duration as a last resort.
fn stream_duration_secs(input: &Path, selector: &str) -> Option<f64> {
    if let Some(d) = ffprobe_field(input, selector, "stream=duration")
        && let Ok(v) = d.parse::<f64>()
    {
        return Some(v);
    }

    if let Some(tag) = ffprobe_field(input, selector, "stream_tags=DURATION")
        && let Some(secs) = parse_hms(&tag)
    {
        return Some(secs);
    }

    if let Some(dts) = ffprobe_field(input, selector, "stream=duration_ts")
        && let Ok(dts) = dts.parse::<f64>()
        && let Some(tb) = ffprobe_field(input, selector, "stream=time_base")
        && let Some((num, den)) = parse_time_base(&tb)
    {
        return Some(dts * num / den);
    }

    if let Some(fmt) = ffprobe_field(input, "", "format=duration")
        && let Ok(v) = fmt.parse::<f64>()
    {
        return Some(v);
    }

    None
}

/// `tempo = a_dur / v_dur` (new_audio_dur = old_audio_dur / tempo). Guards
/// against a non-positive video duration exactly like the bash script.
fn compute_tempo(v_dur: f64, a_dur: f64) -> f64 {
    if v_dur <= 0.0 { 1.0 } else { a_dur / v_dur }
}

fn drift_pct(tempo: f64) -> f64 {
    (tempo - 1.0).abs() * 100.0
}

/// ffmpeg's `atempo` filter only accepts ~0.5..2.0 per instance; chain
/// factors outside that range exactly like `build_atempo_chain()` in bash.
fn build_atempo_chain(tempo: f64) -> String {
    if tempo <= 0.0 {
        return "atempo=1.0".to_string();
    }
    let mut t = tempo;
    let mut parts = Vec::new();
    while t > 2.0 {
        parts.push("atempo=2.0".to_string());
        t /= 2.0;
    }
    while t < 0.5 {
        parts.push("atempo=0.5".to_string());
        t /= 0.5;
    }
    parts.push(format!("atempo={t:.8}"));
    parts.join(",")
}

/// Probe durations, compute the atempo chain, and (unless drift is
/// negligible) spawn the correcting ffmpeg job. `Ok(None)` models the bash
/// no-op path: drift below threshold, no output written, no job started.
pub fn launch(
    input: &Path,
    output: &Path,
    cfg: &Config,
    label: String,
) -> anyhow::Result<Option<PipelineJob>> {
    if !Path::new(FFPROBE).is_file() {
        anyhow::bail!("ffprobe not found: {FFPROBE}");
    }
    if !Path::new(FFMPEG).is_file() {
        anyhow::bail!("ffmpeg not found: {FFMPEG}");
    }
    if !input.is_file() {
        anyhow::bail!("input not found: {}", input.display());
    }

    let v_dur = stream_duration_secs(input, "v:0")
        .ok_or_else(|| anyhow::anyhow!("could not determine video duration"))?;
    let a_dur = stream_duration_secs(input, "a:0")
        .ok_or_else(|| anyhow::anyhow!("could not determine audio duration"))?;

    let tempo = compute_tempo(v_dur, a_dur);
    if drift_pct(tempo) < DRIFT_THRESHOLD_PCT {
        return Ok(None);
    }

    let chain = build_atempo_chain(tempo);
    let mut cmd = Command::new(FFMPEG);
    cmd.args(["-y", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-map", "0:a:0", "-c:v", "copy", "-af"])
        .arg(format!("{chain},aresample=async=1:first_pts=0"))
        .args(["-c:a", "aac", "-b:a", "192k"])
        .arg(output);

    let job = PipelineJob::start_native_single(label, input, cmd, &cfg.log_dir())?;
    Ok(Some(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_parses_and_rejects_na() {
        assert_eq!(parse_hms("00:01:30.500"), Some(90.5));
        assert_eq!(parse_hms("01:00:00"), Some(3600.0));
        assert_eq!(parse_hms("N/A"), None);
    }

    #[test]
    fn time_base_parses() {
        assert_eq!(parse_time_base("1/1000"), Some((1.0, 1000.0)));
        assert_eq!(parse_time_base("1/0"), None);
    }

    #[test]
    fn tempo_matches_bash_formula() {
        assert_eq!(compute_tempo(100.0, 100.0), 1.0);
        assert_eq!(compute_tempo(100.0, 101.0), 1.01);
        assert_eq!(compute_tempo(0.0, 50.0), 1.0); // guarded
        assert_eq!(compute_tempo(-5.0, 50.0), 1.0); // guarded
    }

    #[test]
    fn drift_pct_matches_bash_formula() {
        assert!((drift_pct(1.01) - 1.0).abs() < 1e-9);
        assert!((drift_pct(0.99) - 1.0).abs() < 1e-9);
        assert_eq!(drift_pct(1.0), 0.0);
    }

    #[test]
    fn atempo_chain_within_range_is_single_factor() {
        assert_eq!(build_atempo_chain(1.25), "atempo=1.25000000");
        assert_eq!(build_atempo_chain(0.75), "atempo=0.75000000");
    }

    #[test]
    fn atempo_chain_above_two_splits() {
        // 3.0 -> one atempo=2.0, remainder 1.5
        assert_eq!(build_atempo_chain(3.0), "atempo=2.0,atempo=1.50000000");
        // 5.0 -> two atempo=2.0 (5/2/2 = 1.25), remainder 1.25
        assert_eq!(
            build_atempo_chain(5.0),
            "atempo=2.0,atempo=2.0,atempo=1.25000000"
        );
    }

    #[test]
    fn atempo_chain_below_half_splits() {
        // 0.2 -> 0.2/0.5=0.4 (still <0.5) -> 0.4/0.5=0.8, remainder 0.8
        assert_eq!(
            build_atempo_chain(0.2),
            "atempo=0.5,atempo=0.5,atempo=0.80000000"
        );
    }

    #[test]
    fn atempo_chain_non_positive_is_identity() {
        assert_eq!(build_atempo_chain(0.0), "atempo=1.0");
        assert_eq!(build_atempo_chain(-1.0), "atempo=1.0");
    }

    /// End-to-end smoke test against a real sample with genuine (tiny) A/V
    /// drift. Not run by default (`cargo test -- --ignored`) — needs real
    /// sample media.
    #[test]
    #[ignore]
    fn fix_sync_end_to_end_smoke() {
        let cfg = Config::default();
        let input = Path::new(
            "/tmp/claude-1000/-home-ryan-Videos/129570ca-1234-4fe4-8e4c-d071b8ca2f34/scratchpad/vstest/clip_STABLE.mkv",
        );
        assert!(input.is_file(), "test clip missing: {}", input.display());

        let v_dur = stream_duration_secs(input, "v:0").expect("video duration");
        let a_dur = stream_duration_secs(input, "a:0").expect("audio duration");
        let tempo = compute_tempo(v_dur, a_dur);
        println!(
            "v_dur={v_dur} a_dur={a_dur} tempo={tempo} drift_pct={}",
            drift_pct(tempo)
        );
        assert!(
            drift_pct(tempo) >= DRIFT_THRESHOLD_PCT,
            "expected measurable drift on this sample, got {}%",
            drift_pct(tempo)
        );

        let output = std::env::temp_dir().join("fix_sync_smoke_out.mkv");
        let _ = std::fs::remove_file(&output);

        let mut job = launch(input, &output, &cfg, "smoke-test fix_sync".into())
            .expect("launch failed")
            .expect("expected a job, not a no-op (drift is above threshold)");

        for _ in 0..100 {
            job.poll();
            if job.done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(job.done, "job did not finish in time");
        assert_eq!(job.exit_ok, Some(true), "ffmpeg exited non-zero");
        assert!(
            output.is_file(),
            "output file was not created: {}",
            output.display()
        );

        let out_dur = stream_duration_secs(&output, "a:0").expect("output audio duration");
        // Corrected audio duration should now match the video duration closely.
        assert!(
            (out_dur - v_dur).abs() < 0.1,
            "corrected audio duration {out_dur} not close to video duration {v_dur}"
        );
    }
}
