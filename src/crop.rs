//! Letterbox detection for the Upscale and Viewer Encode buttons.
//!
//! VHS letterbox bars are noisy and dark scenes read as bar, so detection is deliberately narrow:
//! ffmpeg's `cropdetect` runs on a few seconds at many points in the file, and only the *vertical*
//! crop is kept (the width always stays the source width). A file is cropped automatically only
//! when at least [`AGREE_PCT`] percent of the usable samples agree on the same picture height and
//! top line; anything else is reported as mixed and left alone.
//!
//! The pure parts (`parse_cropdetect`, `sample_times`, `decide`, [`Crop`]) are unit-tested. The scan
//! itself ([`CropScan`]) is a background thread polled by the UI, like
//! `audio_cleanup::QuietSpotScan`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::tools::{self, Tool};

/// Samples taken per file.
const SAMPLES: usize = 30;
/// Seconds of video each sample covers.
const SAMPLE_SECS: f64 = 2.0;
/// `cropdetect` black threshold (0-255 luma). Higher than the default 24 because VHS bars are not
/// truly black.
const BLACK_LIMIT: u32 = 40;
/// Share of usable samples that must agree for an automatic crop.
const AGREE_PCT: usize = 80;
/// Two samples agree when their height and top line each differ by at most this many lines.
const TOLERANCE: u32 = 2;
/// Lines kept back on each side of the detected picture, because VHS picture edges flicker.
pub const DEFAULT_MARGIN: u32 = 2;
/// A modal height this close to the source height means there is no letterbox.
const NO_BARS_SLACK: u32 = 8;
/// A crop that removes fewer lines than this is not worth applying.
const MIN_REMOVED: u32 = 16;
/// The smallest picture a crop may leave.
const MIN_HEIGHT: u32 = 64;
/// Fewer usable samples than this give no answer.
const MIN_GOOD_SAMPLES: usize = 3;

/// A crop rectangle in source pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Crop {
    pub w: u32,
    pub h: u32,
    pub x: u32,
    pub y: u32,
}

impl Crop {
    /// The ffmpeg filter, `crop=w:h:x:y`.
    pub fn filter(&self) -> String {
        format!("crop={}:{}:{}:{}", self.w, self.h, self.x, self.y)
    }

    /// The crop must sit inside the source, use even values (4:2:0 video) and leave a real picture.
    pub fn validate(&self, src_w: u32, src_h: u32) -> Result<(), String> {
        if self.w == 0 || self.h < MIN_HEIGHT {
            return Err(format!(
                "crop {}x{} is too small (the picture must stay at least {MIN_HEIGHT} lines tall)",
                self.w, self.h
            ));
        }
        if self.x + self.w > src_w || self.y + self.h > src_h {
            return Err(format!(
                "crop {}x{} at {},{} does not fit inside the {src_w}x{src_h} source",
                self.w, self.h, self.x, self.y
            ));
        }
        if [self.w, self.h, self.x, self.y].iter().any(|v| v % 2 != 0) {
            return Err(format!(
                "crop {}x{} at {},{} must use even values",
                self.w, self.h, self.x, self.y
            ));
        }
        Ok(())
    }
}

/// What one sample measured: the picture height and its top line.
pub type Measure = (u32, u32);

/// What a scan concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CropDecision {
    /// Letterbox found: crop to this. `agree` of `usable` samples support it.
    Crop {
        crop: Crop,
        agree: usize,
        usable: usize,
    },
    /// Full-frame picture.
    NoBars { usable: usize },
    /// Two or more different pictures in one file: no automatic crop. Each mode is
    /// `(height, top, samples)`.
    Mixed {
        first: (u32, u32, usize),
        second: Option<(u32, u32, usize)>,
        usable: usize,
    },
    /// Too few usable samples (a black or unreadable file).
    NoData,
}

/// The sample start times: `SAMPLES` points, evenly spread, keeping clear of the start and end
/// where intros and credits sit (60 s on long files, 5% on short ones).
pub fn sample_times(duration: f64) -> Vec<f64> {
    if duration <= SAMPLE_SECS * 2.0 {
        return vec![0.0];
    }
    let skip = if duration > 600.0 {
        60.0
    } else {
        duration * 0.05
    };
    let first = skip;
    let last = duration - skip - SAMPLE_SECS;
    if last <= first {
        return vec![(duration - SAMPLE_SECS).max(0.0) / 2.0];
    }
    (0..SAMPLES)
        .map(|i| first + (last - first) * i as f64 / (SAMPLES - 1) as f64)
        .collect()
}

/// The most common `crop=W:H:X:Y` in one sample's ffmpeg output, as `(H, Y)`; the width and left
/// offset are ignored. `None` when the sample printed no usable crop.
pub fn parse_cropdetect(text: &str) -> Option<Measure> {
    let mut counts: Vec<(Measure, usize)> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.rsplit_once("crop=").map(|(_, r)| r) else {
            continue;
        };
        let mut nums = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .split(':')
            .map(|n| n.parse::<u32>());
        let (Some(Ok(_w)), Some(Ok(h)), Some(Ok(_x)), Some(Ok(y))) =
            (nums.next(), nums.next(), nums.next(), nums.next())
        else {
            continue;
        };
        match counts.iter_mut().find(|(m, _)| *m == (h, y)) {
            Some((_, n)) => *n += 1,
            None => counts.push(((h, y), 1)),
        }
    }
    // Most frequent; on a tie the taller picture, which crops less.
    counts
        .into_iter()
        .max_by_key(|&((h, _), n)| (n, h))
        .map(|(m, _)| m)
}

/// Samples that agree with `c`.
fn agreeing(good: &[Measure], c: Measure) -> usize {
    good.iter()
        .filter(|&&(h, y)| h.abs_diff(c.0) <= TOLERANCE && y.abs_diff(c.1) <= TOLERANCE)
        .count()
}

/// The measure most samples agree with, and how many do. A tie goes to the taller picture.
fn best_cluster(good: &[Measure]) -> Option<(Measure, usize)> {
    good.iter()
        .map(|&c| (c, agreeing(good, c)))
        .max_by_key(|&((h, _), n)| (n, h))
}

/// Turn the per-sample measurements into a decision. `margin` lines are kept back on both sides.
pub fn decide(samples: &[Option<Measure>], src_w: u32, src_h: u32, margin: u32) -> CropDecision {
    let good: Vec<Measure> = samples.iter().flatten().copied().collect();
    let usable = good.len();
    if usable < MIN_GOOD_SAMPLES {
        return CropDecision::NoData;
    }
    let Some((mode, agree)) = best_cluster(&good) else {
        return CropDecision::NoData;
    };
    if agree * 100 < AGREE_PCT * usable {
        let rest: Vec<Measure> = good
            .iter()
            .copied()
            .filter(|&(h, y)| !(h.abs_diff(mode.0) <= TOLERANCE && y.abs_diff(mode.1) <= TOLERANCE))
            .collect();
        let second = best_cluster(&rest).map(|((h, y), n)| (h, y, n));
        return CropDecision::Mixed {
            first: (mode.0, mode.1, agree),
            second,
            usable,
        };
    }
    if mode.0 + NO_BARS_SLACK >= src_h {
        return CropDecision::NoBars { usable };
    }
    match crop_from(mode, src_w, src_h, margin) {
        Some(crop) => CropDecision::Crop {
            crop,
            agree,
            usable,
        },
        None => CropDecision::NoBars { usable },
    }
}

/// The crop for a detected picture `(height, top)`: full width, `margin` lines kept back on each
/// side, the top line rounded up and the bottom line down to even values. `None` when the result
/// removes too little or leaves too little.
pub fn crop_from(m: Measure, src_w: u32, src_h: u32, margin: u32) -> Option<Crop> {
    let (h, y) = m;
    let top = (y + margin + 1) & !1;
    let bottom = (y + h).saturating_sub(margin) & !1;
    if bottom <= top || bottom > src_h {
        return None;
    }
    let crop = Crop {
        w: src_w & !1,
        h: bottom - top,
        x: 0,
        y: top,
    };
    if src_h.saturating_sub(crop.h) < MIN_REMOVED || crop.validate(src_w, src_h).is_err() {
        return None;
    }
    Some(crop)
}

/// A one-line description for the status bar and panel.
pub fn describe(d: &CropDecision) -> String {
    match d {
        CropDecision::Crop {
            crop,
            agree,
            usable,
        } => format!(
            "Letterbox: {}x{} from line {} ({agree} of {usable} samples agree)",
            crop.w, crop.h, crop.y
        ),
        CropDecision::NoBars { usable } => format!("No letterbox found ({usable} samples)"),
        CropDecision::Mixed {
            first,
            second,
            usable,
        } => {
            let two = second
                .map(|(h, y, n)| format!(" and {h} lines from {y} ({n})"))
                .unwrap_or_default();
            format!(
                "Mixed picture sizes: {} lines from {} ({} of {usable}){two}. Not cropped automatically.",
                first.0, first.1, first.2
            )
        }
        CropDecision::NoData => "Not enough usable frames to detect a letterbox".into(),
    }
}

// ---------------------------------------------------------------------------
// The background scan
// ---------------------------------------------------------------------------

/// A finished scan.
#[derive(Clone, Debug)]
pub struct ScanResult {
    pub decision: CropDecision,
    pub src: (u32, u32),
    /// The per-sample measurements (the decision can be recomputed with another margin).
    pub samples: Vec<Option<Measure>>,
    /// A frame from a sample that supports the decision, for drawing the crop on.
    pub preview: Option<PathBuf>,
}

struct Shared {
    current_pid: Mutex<Option<i32>>,
    cancel_requested: AtomicBool,
    /// `None` while running.
    result: Mutex<Option<Result<ScanResult, String>>>,
}

pub struct CropScan {
    shared: Arc<Shared>,
    started_at: Instant,
}

impl CropScan {
    pub fn start(input: &Path) -> Self {
        let shared = Arc::new(Shared {
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

    /// `Some(_)` exactly once, when the scan has finished.
    pub fn poll(&self) -> Option<Result<ScanResult, String>> {
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
        let s = self.started_at.elapsed().as_secs();
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Run `cmd` in its own process group, recording its pid for cancel, and return its stderr.
fn run_capturing_stderr(
    mut cmd: std::process::Command,
    shared: &Shared,
) -> Result<std::process::Output, String> {
    use std::os::unix::process::CommandExt as _;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start ffmpeg: {e}"))?;
    *shared.current_pid.lock().unwrap() = Some(child.id() as i32);
    let out = child.wait_with_output();
    *shared.current_pid.lock().unwrap() = None;
    out.map_err(|e| format!("ffmpeg failed: {e}"))
}

/// `(duration, width, height)` of the first video stream.
fn probe(input: &Path) -> Result<(f64, u32, u32), String> {
    let out = tools::command(Tool::Ffprobe)
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=width,height:format=duration"])
        .args(["-of", "default=nw=1:nk=1"])
        .arg(input)
        .output()
        .map_err(|e| format!("failed to start ffprobe: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let w = lines.next().and_then(|v| v.trim().parse().ok());
    let h = lines.next().and_then(|v| v.trim().parse().ok());
    let d = lines.next().and_then(|v| v.trim().parse().ok());
    match (d, w, h) {
        (Some(d), Some(w), Some(h)) => Ok((d, w, h)),
        _ => Err("could not read the video size and duration".into()),
    }
}

fn cache_dir() -> PathBuf {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache")
        });
    root.join("vhs-gui/crop")
}

fn run_scan(input: &Path, shared: &Shared) -> Result<ScanResult, String> {
    tools::require(Tool::Ffmpeg).map_err(|e| e.to_string())?;
    tools::require(Tool::Ffprobe).map_err(|e| e.to_string())?;
    if !input.is_file() {
        return Err(format!("input not found: {}", input.display()));
    }
    let (duration, src_w, src_h) = probe(input)?;
    let times = sample_times(duration);

    let mut samples = Vec::with_capacity(times.len());
    for t in &times {
        if shared.cancel_requested.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let mut cmd = tools::command(Tool::Ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-ss", &format!("{t:.3}")])
            .args(["-t", &SAMPLE_SECS.to_string(), "-i"])
            .arg(input)
            .args(["-an", "-vf"])
            .arg(format!("cropdetect=limit={BLACK_LIMIT}:round=2:reset=0"))
            .args(["-f", "null", "-"]);
        let out = run_capturing_stderr(cmd, shared)?;
        samples.push(parse_cropdetect(&String::from_utf8_lossy(&out.stderr)));
    }
    if shared.cancel_requested.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }

    let decision = decide(&samples, src_w, src_h, DEFAULT_MARGIN);
    let preview = preview_time(&decision, &samples, &times)
        .and_then(|t| extract_preview(input, t, shared).ok());
    Ok(ScanResult {
        decision,
        src: (src_w, src_h),
        samples,
        preview,
    })
}

/// The time of the middle sample among those that support the decision.
fn preview_time(d: &CropDecision, samples: &[Option<Measure>], times: &[f64]) -> Option<f64> {
    let CropDecision::Crop { crop, .. } = d else {
        return None;
    };
    let supporting: Vec<f64> = samples
        .iter()
        .zip(times)
        .filter_map(|(m, &t)| {
            let (h, y) = (*m)?;
            // The crop sits `DEFAULT_MARGIN` inside the picture, so compare with a little slack.
            (y.abs_diff(crop.y) <= TOLERANCE + DEFAULT_MARGIN + 1
                && (y + h).abs_diff(crop.y + crop.h) <= TOLERANCE + DEFAULT_MARGIN + 1)
                .then_some(t)
        })
        .collect();
    supporting.get(supporting.len() / 2).copied()
}

fn extract_preview(input: &Path, t: f64, shared: &Shared) -> Result<PathBuf, String> {
    let dir = cache_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frame".into());
    let out = dir.join(format!("{stem}.jpg"));
    let mut cmd = tools::command(Tool::Ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
        .args(["-ss", &format!("{t:.3}"), "-i"])
        .arg(input)
        .args(["-frames:v", "1", "-q:v", "2"])
        .arg(&out);
    let res = run_capturing_stderr(cmd, shared)?;
    if res.status.success() && out.is_file() {
        Ok(out)
    } else {
        Err("could not extract a preview frame".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(v: &[(u32, u32)]) -> Vec<Option<Measure>> {
        v.iter().map(|&m| Some(m)).collect()
    }

    #[test]
    fn parses_the_most_common_crop_and_ignores_width_and_left_offset() {
        let text = "\
[Parsed_cropdetect_0 @ 0x1] x1:0 x2:719 y1:96 y2:375 w:720 h:280 x:0 y:96 pts:1 t:0.03 limit:0.156 crop=720:280:0:96
[Parsed_cropdetect_0 @ 0x1] x1:2 x2:719 y1:96 y2:375 w:718 h:280 x:2 y:96 pts:2 t:0.06 limit:0.156 crop=718:280:2:96
[Parsed_cropdetect_0 @ 0x1] x1:0 x2:719 y1:0 y2:479 w:720 h:480 x:0 y:0 pts:3 t:0.1 limit:0.156 crop=720:478:0:0
";
        assert_eq!(parse_cropdetect(text), Some((280, 96)));
    }

    #[test]
    fn a_tie_goes_to_the_taller_picture() {
        let text = "crop=720:200:0:100\ncrop=720:280:0:96\n";
        assert_eq!(parse_cropdetect(text), Some((280, 96)));
    }

    #[test]
    fn output_without_a_crop_line_gives_none() {
        assert_eq!(parse_cropdetect("frame=0 fps=0\ncrop=\ncrop=ab:cd\n"), None);
        assert_eq!(parse_cropdetect(""), None);
    }

    #[test]
    fn sample_times_skip_the_ends_of_a_long_file() {
        let t = sample_times(9015.0);
        assert_eq!(t.len(), SAMPLES);
        assert!((t[0] - 60.0).abs() < 1e-9);
        assert!(*t.last().unwrap() <= 9015.0 - 60.0 - SAMPLE_SECS + 1e-9);
        assert!(t.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn sample_times_use_five_percent_on_a_short_file() {
        let t = sample_times(200.0);
        assert!((t[0] - 10.0).abs() < 1e-9);
        assert!(*t.last().unwrap() <= 190.0);
    }

    #[test]
    fn sample_times_cope_with_tiny_files() {
        assert_eq!(sample_times(3.0), vec![0.0]);
        let t = sample_times(5.0);
        assert!(t.iter().all(|&v| (0.0..5.0).contains(&v)));
    }

    #[test]
    fn a_consistent_letterbox_gives_a_crop_with_the_margin_kept_back() {
        // The measured tape: 280 lines at line 96.
        let d = decide(&some(&[(280, 96); 28]), 720, 480, 2);
        assert_eq!(
            d,
            CropDecision::Crop {
                crop: Crop {
                    w: 720,
                    h: 276,
                    x: 0,
                    y: 98
                },
                agree: 28,
                usable: 28,
            }
        );
    }

    #[test]
    fn a_few_stray_samples_do_not_stop_the_crop() {
        let mut s = some(&[(280, 96); 26]);
        s.push(Some((188, 96))); // a dark scene
        s.push(Some((478, 0))); // a title card
        s.push(None); // a failed sample
        s.push(None);
        match decide(&s, 720, 480, 2) {
            CropDecision::Crop {
                crop,
                agree,
                usable,
            } => {
                assert_eq!((crop.h, crop.y), (276, 98));
                assert_eq!((agree, usable), (26, 28));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn small_differences_in_the_bar_edge_count_as_agreement() {
        let mut s = some(&[(280, 96); 10]);
        s.extend(some(&[(278, 98); 6]));
        s.extend(some(&[(282, 96); 4]));
        assert!(matches!(
            decide(&s, 720, 480, 2),
            CropDecision::Crop { agree: 20, .. }
        ));
    }

    #[test]
    fn a_full_frame_file_has_no_bars() {
        let d = decide(&some(&[(478, 0); 30]), 720, 480, 2);
        assert_eq!(d, CropDecision::NoBars { usable: 30 });
    }

    #[test]
    fn a_file_with_two_picture_sizes_is_mixed() {
        let mut s = some(&[(280, 96); 18]);
        s.extend(some(&[(478, 0); 12]));
        match decide(&s, 720, 480, 2) {
            CropDecision::Mixed {
                first,
                second,
                usable,
            } => {
                assert_eq!(first, (280, 96, 18));
                assert_eq!(second, Some((478, 0, 12)));
                assert_eq!(usable, 30);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn too_few_usable_samples_give_no_answer() {
        assert_eq!(
            decide(&[None, None, None], 720, 480, 2),
            CropDecision::NoData
        );
        assert_eq!(
            decide(&some(&[(280, 96), (280, 96)]), 720, 480, 2),
            CropDecision::NoData
        );
    }

    #[test]
    fn a_crop_that_removes_almost_nothing_is_not_applied() {
        // 472 of 480 lines: inside NO_BARS_SLACK, so no bars.
        assert_eq!(
            decide(&some(&[(472, 4); 10]), 720, 480, 2),
            CropDecision::NoBars { usable: 10 }
        );
    }

    #[test]
    fn crop_from_rounds_to_even_lines_and_never_exceeds_the_source() {
        // Odd top: 97 + 2 = 99 rounds up to 100; bottom 97 + 281 - 2 = 376.
        let c = crop_from((281, 97), 720, 480, 2).unwrap();
        assert_eq!((c.y, c.h), (100, 276));
        assert!(c.y.is_multiple_of(2) && c.h.is_multiple_of(2) && c.y + c.h <= 480);
        // A margin that eats the picture gives nothing.
        assert_eq!(crop_from((280, 96), 720, 480, 200), None);
        // A picture running past the bottom of the frame gives nothing.
        assert_eq!(crop_from((400, 200), 720, 480, 2), None);
    }

    #[test]
    fn a_zero_margin_keeps_the_detected_rectangle() {
        let c = crop_from((280, 96), 720, 480, 0).unwrap();
        assert_eq!((c.h, c.y), (280, 96));
    }

    #[test]
    fn crop_filter_text_and_validation() {
        let c = Crop {
            w: 720,
            h: 276,
            x: 0,
            y: 98,
        };
        assert_eq!(c.filter(), "crop=720:276:0:98");
        assert!(c.validate(720, 480).is_ok());
        assert!(c.validate(720, 360).is_err()); // runs past the bottom
        assert!(Crop { h: 275, ..c }.validate(720, 480).is_err()); // odd
        assert!(Crop { h: 32, ..c }.validate(720, 480).is_err()); // too small
        assert!(Crop { w: 800, ..c }.validate(720, 480).is_err()); // too wide
    }

    #[test]
    fn descriptions_name_the_result() {
        let d = decide(&some(&[(280, 96); 10]), 720, 480, 2);
        assert_eq!(
            describe(&d),
            "Letterbox: 720x276 from line 98 (10 of 10 samples agree)"
        );
        assert!(describe(&CropDecision::NoBars { usable: 9 }).starts_with("No letterbox"));
    }

    /// Builds an 8-second full-frame intro followed by 80 seconds of a 280-line picture with
    /// black bars, and a full-frame clip, then scans both with real ffmpeg. Not run by default:
    /// it spawns ffmpeg and writes about 40 MB (under the cache dir, because /tmp is RAM).
    #[test]
    #[ignore]
    fn scan_finds_a_letterbox_after_a_full_frame_intro() {
        let dir = cache_dir().join("test-scan");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ffmpeg = |args: &[&str], out: &Path| {
            let st = tools::command(Tool::Ffmpeg)
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
                .args(args)
                .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "16"])
                .args(["-pix_fmt", "yuv420p"])
                .arg(out)
                .status()
                .expect("ffmpeg");
            assert!(st.success());
        };

        let letterboxed = dir.join("letterboxed.mkv");
        ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=720x480:rate=30:duration=8",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=720x280:rate=30:duration=80,pad=720:480:0:100:black",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                "-map",
                "[v]",
            ],
            &letterboxed,
        );
        let full = dir.join("full.mkv");
        ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=720x480:rate=30:duration=60",
            ],
            &full,
        );

        let run = |p: &Path| {
            let scan = CropScan::start(p);
            loop {
                if let Some(r) = scan.poll() {
                    break r.expect("scan should not fail");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        };

        let r = run(&letterboxed);
        match r.decision {
            CropDecision::Crop {
                crop,
                agree,
                usable,
            } => {
                // Bars are 100 above and below the 280-line picture; 2 lines are kept back.
                assert_eq!((crop.y, crop.h), (102, 276), "{crop:?}");
                assert!(agree * 100 >= 80 * usable);
            }
            other => panic!("{other:?}"),
        }
        assert!(r.preview.as_deref().is_some_and(Path::is_file));

        assert!(matches!(run(&full).decision, CropDecision::NoBars { .. }));
        let _ = fs::remove_dir_all(&dir);
    }
}
