//! Live capture. ffmpeg is spawned directly; this replaced `vhs_capture_ffmpeg.sh`.
//!
//! What the script did, and what happens now:
//!   * It sourced `~/Videos/ffmpeg-current/capture.env` for the hardware and codec settings. Those
//!     are compiled in as [`CaptureSettings::defaults`] (identical to the shipped archival
//!     `capture.env`); the same file, if present, still overrides them so switching slots with
//!     `vhs_mode.sh` keeps working, and a fresh install needs no file.
//!   * It wrote a pgid file for the GUI to signal and ran ffmpeg through `tee` into the log. Now the
//!     child is held directly, put in its own process group so a signal never reaches vhs-gui, and
//!     ffmpeg's stderr goes straight into the log file. That keeps the old property that a capture
//!     (and its log) carries on to the `-t` backstop if the GUI dies.
//!   * Its "is ffmpeg still running" cleanup and the closing `ffprobe` listing are not needed.
//!
//! The ffmpeg arguments come from the pure [`ffmpeg_args`], compared against the real script by the
//! ignored `matches_the_bash_script` test.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::clock;
use crate::config::Config;
use crate::tools::{self, Tool};

/// Fixed ffmpeg `-t` backstop passed at spawn time, independent of the
/// user-facing "Cap"/"Stop after" duration. ffmpeg's own `-t` can't be moved
/// once the process is running, so the user-adjustable duration is enforced
/// entirely via `arm_stop_timer`'s SIGINT instead; this only fires if the GUI
/// itself dies mid-capture and never gets to arm or re-arm that timer.
pub const FFMPEG_HARD_SAFETY_CAP: &str = "06:00:00";

/// Where the capture sends its low-latency preview stream and where the monitor panel's mpv
/// listens.
pub const PREVIEW_URL: &str = "udp://127.0.0.1:23000?pkt_size=1316";

/// Everything the script took from `capture.env`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSettings {
    pub v4l2_dev: String,
    pub size: String,
    pub fps: String,
    pub input_fmt: String,
    pub alsa_dev: String,
    pub ar: String,
    pub ac: String,
    pub outdir: PathBuf,
    pub prefix: String,
    pub vcodec: String,
    pub pixfmt: String,
    pub ffv1_level: String,
    pub ffv1_slices: String,
    pub x264_preset: String,
    pub x264_crf: String,
    pub color_range: String,
    pub color_prim: String,
    pub color_trc: String,
    pub color_space: String,
    pub preview_scale: String,
    pub preview_url: String,
}

impl CaptureSettings {
    /// The shipped archival slot: MacroSilicon USB grabber, 720x480 YUYV at 30 fps, FFV1 4:2:2 with
    /// PCM audio, SMPTE 170M tags.
    pub fn defaults(cfg: &Config) -> Self {
        Self {
            v4l2_dev: cfg.v4l2_device.clone(),
            size: "720x480".into(),
            fps: "30".into(),
            input_fmt: "yuyv422".into(),
            alsa_dev: "hw:CARD=MS210x,DEV=0".into(),
            ar: "48000".into(),
            ac: "2".into(),
            outdir: cfg.archival_dir(),
            prefix: "VHS_ARCHIVAL".into(),
            vcodec: "ffv1".into(),
            pixfmt: "yuv422p".into(),
            ffv1_level: "3".into(),
            ffv1_slices: "16".into(),
            x264_preset: "veryfast".into(),
            x264_crf: "18".into(),
            color_range: "tv".into(),
            color_prim: "smpte170m".into(),
            color_trc: "smpte170m".into(),
            color_space: "smpte170m".into(),
            preview_scale: "480:360".into(),
            preview_url: PREVIEW_URL.into(),
        }
    }

    /// Apply `KEY=value` lines from a `capture.env`. Handles `#` comments, an `export` prefix,
    /// single or double quotes and `$HOME`/`${HOME}`; ignores keys it does not know (such as
    /// `VHS_FFMPEG_BIN`, which the tool resolver replaced).
    pub fn apply_env_text(&mut self, text: &str, home: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let Some((key, raw)) = line.split_once('=') else {
                continue;
            };
            let raw = raw.trim();
            let value = match (raw.chars().next(), raw.chars().last()) {
                (Some(a @ ('"' | '\'')), Some(b)) if a == b && raw.len() >= 2 => {
                    &raw[1..raw.len() - 1]
                }
                _ => raw,
            };
            let value = value.replace("${HOME}", home).replace("$HOME", home);
            let slot = match key.trim() {
                "VHS_V4L2_DEV" => &mut self.v4l2_dev,
                "VHS_SIZE" => &mut self.size,
                "VHS_FPS" => &mut self.fps,
                "VHS_INPUT_FMT" => &mut self.input_fmt,
                "VHS_ALSA_DEV" => &mut self.alsa_dev,
                "VHS_AR" => &mut self.ar,
                "VHS_AC" => &mut self.ac,
                "VHS_PREFIX" => &mut self.prefix,
                "VHS_VCODEC" => &mut self.vcodec,
                "VHS_PIXFMT" => &mut self.pixfmt,
                "VHS_FFV1_LEVEL" => &mut self.ffv1_level,
                "VHS_FFV1_SLICES" => &mut self.ffv1_slices,
                "VHS_X264_PRESET" => &mut self.x264_preset,
                "VHS_X264_CRF" => &mut self.x264_crf,
                "VHS_COLOR_RANGE" => &mut self.color_range,
                "VHS_COLOR_PRIM" => &mut self.color_prim,
                "VHS_COLOR_TRC" => &mut self.color_trc,
                "VHS_COLOR_SPACE" => &mut self.color_space,
                "VHS_PREVIEW_SCALE" => &mut self.preview_scale,
                "VHS_PREVIEW_URL" => &mut self.preview_url,
                "VHS_OUTDIR" => {
                    self.outdir = PathBuf::from(value);
                    continue;
                }
                _ => continue,
            };
            *slot = value;
        }
    }

    /// Defaults, then the active slot's `capture.env` (`~/Videos/ffmpeg-current/capture.env`) over
    /// them when it exists.
    pub fn load(cfg: &Config) -> Self {
        let mut s = Self::defaults(cfg);
        let home = std::env::var("HOME").unwrap_or_default();
        if let Ok(text) = fs::read_to_string(cfg.videos_dir.join("ffmpeg-current/capture.env")) {
            s.apply_env_text(&text, &home);
        }
        s
    }
}

/// The ffmpeg command line, in the script's order. `preview` adds the second, low-latency UDP
/// output (the GUI always asked for it).
pub fn ffmpeg_args(
    s: &CaptureSettings,
    cap: &str,
    out: &Path,
    preview: bool,
) -> anyhow::Result<Vec<OsString>> {
    let mut a: Vec<OsString> = Vec::new();
    let mut add = |items: &[&str]| a.extend(items.iter().map(OsString::from));

    add(&["-hide_banner", "-nostdin"]);
    add(&["-f", "v4l2", "-use_wallclock_as_timestamps", "1"]);
    add(&["-thread_queue_size", "1024", "-input_format", &s.input_fmt]);
    add(&[
        "-video_size",
        &s.size,
        "-framerate",
        &s.fps,
        "-i",
        &s.v4l2_dev,
    ]);
    add(&["-f", "alsa", "-use_wallclock_as_timestamps", "1"]);
    add(&["-thread_queue_size", "1024", "-ar", &s.ar, "-ac", &s.ac]);
    add(&["-i", &s.alsa_dev]);
    add(&["-map", "0:v:0", "-map", "1:a:0", "-c:a", "pcm_s16le"]);
    match s.vcodec.as_str() {
        "ffv1" => {
            add(&["-c:v", "ffv1", "-level:v", &s.ffv1_level, "-g:v", "1"]);
            add(&["-slices:v", &s.ffv1_slices, "-slicecrc:v", "1"]);
            add(&["-pix_fmt", &s.pixfmt]);
        }
        "libx264" => {
            add(&["-c:v", "libx264", "-preset", &s.x264_preset]);
            add(&["-crf", &s.x264_crf, "-pix_fmt", &s.pixfmt]);
        }
        other => anyhow::bail!("unsupported VHS_VCODEC '{other}' (expected ffv1 or libx264)"),
    }
    add(&[
        "-color_range",
        &s.color_range,
        "-color_primaries",
        &s.color_prim,
    ]);
    add(&["-color_trc", &s.color_trc, "-colorspace", &s.color_space]);
    add(&["-max_interleave_delta", "0", "-fflags", "+genpts"]);
    add(&["-fps_mode", "cfr", "-t", cap]);
    a.push(out.into());
    if preview {
        // `-t` is repeated on purpose: ffmpeg output options apply to the next output only, so
        // without it the preview has no limit and keeps the process alive after the archival file
        // closes.
        let vf = format!("scale={}", s.preview_scale);
        let mut add = |items: &[&str]| a.extend(items.iter().map(OsString::from));
        add(&["-map", "0:v:0", "-vf", &vf]);
        add(&[
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
        ]);
        add(&["-g", "15", "-pix_fmt", "yuv420p", "-an", "-t", cap]);
        add(&["-f", "mpegts", &s.preview_url]);
    }
    Ok(a)
}

#[derive(Clone, Debug, Default)]
pub struct CaptureStats {
    pub frame: u64,
    pub time: String,
    pub bitrate: String,
    pub elapsed: Duration,
}

/// Latest progress from a chunk of ffmpeg stderr. ffmpeg separates progress updates with `\r`, so
/// split on that as well as `\n`, and ignore a trailing segment that may still be half written.
pub fn parse_progress(text: &str) -> Option<(u64, String, String)> {
    let end = text.rfind(['\r', '\n'])?;
    let seg = text[..end]
        .rsplit(['\r', '\n'])
        .find(|s| s.contains("frame="))?;
    let frame = field(seg, "frame=")?.parse().ok()?;
    Some((
        frame,
        field(seg, "time=").unwrap_or("").to_owned(),
        field(seg, "bitrate=").unwrap_or("").to_owned(),
    ))
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = &line[line.find(key)? + key.len()..];
    Some(
        rest.split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/'),
    )
}

/// How much of the log's end to read for progress. A capture log grows for hours; reading it whole
/// on every UI poll would not do.
const TAIL_BYTES: u64 = 8 * 1024;

fn tail_progress(log: &Path) -> Option<(u64, String, String)> {
    let mut f = fs::File::open(log).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    parse_progress(&String::from_utf8_lossy(&buf))
}

/// Send SIGINT to `armed` only if it is still the live capture's process group. The check and the
/// signal happen under the lock that `poll` takes to clear the pgid after reaping the child, so a
/// late-firing timer can never signal a later, unrelated process group.
fn signal_if_live(live: &Mutex<Option<i32>>, armed: i32, send: impl FnOnce(i32)) {
    let guard = live.lock().unwrap();
    if *guard == Some(armed) {
        send(armed);
    }
}

fn sigint_group(pgid: i32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let _ = killpg(Pid::from_raw(pgid), Signal::SIGINT);
}

pub struct CaptureController {
    child: Option<Child>,
    /// Process group of the running capture, `None` when idle. Shared with the stop timer.
    live_pgid: Arc<Mutex<Option<i32>>>,
    log_path: Option<PathBuf>,
    started_at: Option<Instant>,
    expected_output: Option<PathBuf>,
    /// The archival file, once ffmpeg has created it.
    pub output_path: Option<PathBuf>,
    pub stats: Arc<Mutex<CaptureStats>>,
    /// Cancellation flag for the OS-level stop timer thread, if one is armed.
    stop_timer_cancel: Option<Arc<AtomicBool>>,
}

impl CaptureController {
    pub fn new() -> Self {
        Self {
            child: None,
            live_pgid: Arc::new(Mutex::new(None)),
            log_path: None,
            started_at: None,
            expected_output: None,
            output_path: None,
            stats: Arc::new(Mutex::new(CaptureStats::default())),
            stop_timer_cancel: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    pub fn start(&mut self, cfg: &Config, max_duration: &str) -> anyhow::Result<()> {
        let ffmpeg = tools::require(Tool::Ffmpeg)?;
        let settings = CaptureSettings::load(cfg);
        self.start_with(&ffmpeg, &settings, &cfg.log_dir(), max_duration)
    }

    /// `start` with the program and settings spelled out (tests substitute a stub for ffmpeg).
    fn start_with(
        &mut self,
        program: &Path,
        settings: &CaptureSettings,
        log_dir: &Path,
        max_duration: &str,
    ) -> anyhow::Result<()> {
        if self.child.is_some() {
            anyhow::bail!("capture already running");
        }
        let ts = clock::stamp();
        let out = settings
            .outdir
            .join(format!("{}_{ts}.mkv", settings.prefix));
        let log = log_dir.join(format!("{}_{ts}.ffmpeg.log", settings.prefix));
        let args = ffmpeg_args(settings, max_duration, &out, true)?;
        fs::create_dir_all(&settings.outdir)?;
        fs::create_dir_all(log_dir)?;
        let log_file = fs::File::create(&log)?;

        // process_group(0) gives the child its own group (PGID == its PID). Without it the child
        // would share vhs-gui's group and a group signal would kill us too, which corrupted
        // KWin's EGL state.
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log_file))
            .process_group(0)
            .spawn()?;

        *self.live_pgid.lock().unwrap() = Some(child.id() as i32);
        self.started_at = Some(Instant::now());
        self.expected_output = Some(out);
        self.output_path = None;
        self.log_path = Some(log);
        *self.stats.lock().unwrap() = CaptureStats::default();
        self.child = Some(child);
        Ok(())
    }

    /// Call periodically from the UI thread to reap the child and refresh the stats.
    pub fn poll(&mut self) {
        let mut exited = false;
        {
            // Reap and clear the live pgid under the lock the stop timer takes, so the two cannot
            // interleave (see `signal_if_live`).
            let mut live = self.live_pgid.lock().unwrap();
            if let Some(child) = self.child.as_mut() {
                match child.try_wait() {
                    Ok(None) => {}
                    Ok(Some(_)) | Err(_) => {
                        self.child = None;
                        self.started_at = None;
                        *live = None;
                        exited = true;
                    }
                }
            }
        }
        if let Some(started) = self.started_at
            && let Ok(mut s) = self.stats.lock()
        {
            s.elapsed = started.elapsed();
        }
        if self.output_path.is_none()
            && let Some(p) = &self.expected_output
            && p.is_file()
        {
            self.output_path = Some(p.clone());
        }
        // Progress comes from the log's tail while running, plus one last read at exit.
        if (self.child.is_some() || exited)
            && let Some(log) = &self.log_path
            && let Some((frame, time, bitrate)) = tail_progress(log)
            && let Ok(mut s) = self.stats.lock()
        {
            s.frame = frame;
            s.time = time;
            s.bitrate = bitrate;
        }
    }

    pub fn stop(&mut self) {
        self.cancel_stop_timer();
        let live = *self.live_pgid.lock().unwrap();
        if let Some(pgid) = live {
            sigint_group(pgid);
        }
    }

    /// Arm an OS-level timer that sends SIGINT to the capture process group after
    /// `secs` seconds, fully independent of the egui/winit event loop. Cancels
    /// any previously armed timer first.
    ///
    /// The timer binds to the capture's process group at arm time and signals only if that
    /// group is still the live one (see `signal_if_live`).
    pub fn arm_stop_timer(&mut self, secs: u64) {
        self.cancel_stop_timer();
        let Some(armed) = *self.live_pgid.lock().unwrap() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = Arc::clone(&cancel);
        let live = Arc::clone(&self.live_pgid);
        // Wall-clock deadline: fires correctly even after a system suspend/resume.
        let deadline = SystemTime::now() + Duration::from_secs(secs);

        thread::spawn(move || {
            // Sleep in <= 500 ms chunks so the cancel flag is checked regularly.
            loop {
                if cancel_thread.load(Ordering::Relaxed) {
                    return;
                }
                let remaining = deadline
                    .duration_since(SystemTime::now())
                    .unwrap_or(Duration::ZERO);
                if remaining.is_zero() {
                    break;
                }
                thread::sleep(remaining.min(Duration::from_millis(500)));
            }
            if !cancel_thread.load(Ordering::Relaxed) {
                signal_if_live(&live, armed, sigint_group);
            }
        });
        self.stop_timer_cancel = Some(cancel);
    }

    /// Cancel a previously armed stop timer, if any.
    pub fn cancel_stop_timer(&mut self) {
        if let Some(flag) = self.stop_timer_cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn elapsed_str(&self) -> String {
        if let Some(started) = self.started_at {
            let d = started.elapsed();
            let h = d.as_secs() / 3600;
            let m = (d.as_secs() % 3600) / 60;
            let s = d.as_secs() % 60;
            format!("{h:02}:{m:02}:{s:02}")
        } else {
            "--:--:--".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
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

    fn cfg() -> Config {
        Config::default()
    }

    fn strings(a: &[OsString]) -> Vec<String> {
        a.iter().map(|x| x.to_string_lossy().into_owned()).collect()
    }

    // ---- settings and arguments ----

    #[test]
    fn default_arguments_are_the_archival_slots() {
        let s = CaptureSettings::defaults(&cfg());
        let args = ffmpeg_args(&s, "06:00:00", Path::new("/o/a.mkv"), true).unwrap();
        let want = "-hide_banner -nostdin \
            -f v4l2 -use_wallclock_as_timestamps 1 -thread_queue_size 1024 -input_format yuyv422 \
            -video_size 720x480 -framerate 30 \
            -i /dev/v4l/by-id/usb-MACROSIL_AV_TO_USB2.0-video-index0 \
            -f alsa -use_wallclock_as_timestamps 1 -thread_queue_size 1024 -ar 48000 -ac 2 \
            -i hw:CARD=MS210x,DEV=0 \
            -map 0:v:0 -map 1:a:0 -c:a pcm_s16le \
            -c:v ffv1 -level:v 3 -g:v 1 -slices:v 16 -slicecrc:v 1 -pix_fmt yuv422p \
            -color_range tv -color_primaries smpte170m -color_trc smpte170m -colorspace smpte170m \
            -max_interleave_delta 0 -fflags +genpts -fps_mode cfr -t 06:00:00 /o/a.mkv \
            -map 0:v:0 -vf scale=480:360 -c:v libx264 -preset ultrafast -tune zerolatency \
            -g 15 -pix_fmt yuv420p -an -t 06:00:00 -f mpegts udp://127.0.0.1:23000?pkt_size=1316";
        assert_eq!(strings(&args).join(" "), want);
    }

    #[test]
    fn without_preview_there_is_a_single_output() {
        let s = CaptureSettings::defaults(&cfg());
        let args = strings(&ffmpeg_args(&s, "01:00:00", Path::new("/o/a.mkv"), false).unwrap());
        assert_eq!(args.last().unwrap(), "/o/a.mkv");
        assert!(!args.contains(&"mpegts".to_string()));
    }

    #[test]
    fn x264_slot_uses_its_own_codec_arguments() {
        let mut s = CaptureSettings::defaults(&cfg());
        s.vcodec = "libx264".into();
        s.pixfmt = "yuv420p".into();
        let args = strings(&ffmpeg_args(&s, "01:00:00", Path::new("/o/a.mkv"), false).unwrap());
        let joined = args.join(" ");
        assert!(
            joined.contains("-c:v libx264 -preset veryfast -crf 18 -pix_fmt yuv420p"),
            "{joined}"
        );
        assert!(!joined.contains("ffv1"));
    }

    #[test]
    fn unsupported_codec_is_an_error() {
        let mut s = CaptureSettings::defaults(&cfg());
        s.vcodec = "prores".into();
        let err = ffmpeg_args(&s, "01:00:00", Path::new("/o/a.mkv"), true).unwrap_err();
        assert!(err.to_string().contains("prores"), "{err}");
    }

    #[test]
    fn env_text_parsing() {
        let mut s = CaptureSettings::defaults(&cfg());
        s.apply_env_text(
            "# a comment\n\nVHS_SIZE=\"640x480\"\nexport VHS_FPS=25\n\
             VHS_OUTDIR=\"$HOME/Videos/captures/other\"\nVHS_PREFIX='X'\n\
             VHS_FFMPEG_BIN=\"/usr/bin/ffmpeg\"\nVHS_UNKNOWN=1\nnot a line\n\
             VHS_ALSA_DEV=\"hw:CARD=X,DEV=0\"\n",
            "/home/u",
        );
        assert_eq!(s.size, "640x480");
        assert_eq!(s.fps, "25");
        assert_eq!(s.prefix, "X");
        assert_eq!(s.outdir, PathBuf::from("/home/u/Videos/captures/other"));
        assert_eq!(s.alsa_dev, "hw:CARD=X,DEV=0");
        // Untouched and unknown keys leave the defaults alone.
        assert_eq!(s.vcodec, "ffv1");
    }

    // ---- progress parsing (the format real capture logs have) ----

    const REAL_CHUNK: &str = "s/s dup=2 drop=0 speed=   1x elapsed=0:03:51.53    \r\
        frame= 6962 fps= 30 q=-0.0 q=25.0 size= 1172224KiB time=00:03:52.03 bitrate=41385.9kbits/s dup=2 drop=0 speed=   1x elapsed=0:03:52.03    \r\
        frame=";

    #[test]
    fn progress_takes_the_last_complete_carriage_return_segment() {
        let (frame, time, bitrate) = parse_progress(REAL_CHUNK).unwrap();
        assert_eq!(frame, 6962);
        assert_eq!(time, "00:03:52.03");
        assert_eq!(bitrate, "41385.9kbits/s");
    }

    #[test]
    fn progress_ignores_text_without_a_complete_update() {
        assert_eq!(parse_progress(""), None);
        assert_eq!(parse_progress("frame= 12 fps"), None); // no terminator yet
        assert_eq!(parse_progress("Stream mapping:\nPress [q]\n"), None);
    }

    #[test]
    fn tail_reads_only_the_end_of_a_big_log() {
        let dir = TempDir::new("capture-tail");
        let log = dir.0.join("c.ffmpeg.log");
        // 200 KB of old output, then the real tail.
        let mut body = "x".repeat(200_000);
        body.push('\n');
        body.push_str(REAL_CHUNK);
        fs::write(&log, body).unwrap();
        assert_eq!(tail_progress(&log).unwrap().0, 6962);
        assert_eq!(tail_progress(&dir.0.join("missing.log")), None);
    }

    // ---- the stop-timer guard ----

    #[test]
    fn a_timer_signals_only_the_live_group() {
        let live = Mutex::new(Some(100));
        let mut sent = Vec::new();
        signal_if_live(&live, 100, |p| sent.push(p));
        assert_eq!(sent, [100]);

        let mut sent = Vec::new();
        signal_if_live(&live, 101, |p| sent.push(p)); // a different, stale group
        *live.lock().unwrap() = None; // capture finished
        signal_if_live(&live, 100, |p| sent.push(p));
        assert!(sent.is_empty(), "{sent:?}");
    }

    // ---- the controller against a stub ffmpeg ----

    /// A stand-in for ffmpeg: prints one progress update, then idles until SIGINT, which it
    /// handles by logging and exiting cleanly (as ffmpeg finalizes the file).
    fn stub_ffmpeg(dir: &Path) -> PathBuf {
        let p = dir.join("ffmpeg-stub");
        fs::write(
            &p,
            "#!/bin/sh\n\
             trap 'echo interrupted >&2; exit 0' INT\n\
             printf 'frame=   12 fps= 30 q=-0.0 size=       1KiB time=00:00:00.40 bitrate=  20.0kbits/s speed=1x    \\r' >&2\n\
             while :; do sleep 0.05; done\n",
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn settings_in(dir: &Path) -> CaptureSettings {
        let mut s = CaptureSettings::defaults(&cfg());
        s.outdir = dir.join("archival");
        s
    }

    /// Start the controller on a freshly written stub script. Tests run in parallel, and a fork in
    /// one test can inherit another's still-open write handle on its stub, so `exec` may briefly
    /// fail with "Text file busy" (ETXTBSY); retry that one error.
    fn start_stub(c: &mut CaptureController, stub: &Path, dir: &Path) {
        for _ in 0..100 {
            match c.start_with(stub, &settings_in(dir), &dir.join("logs"), "06:00:00") {
                Ok(()) => return,
                Err(e) if e.to_string().contains("Text file busy") => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("start_with: {e}"),
            }
        }
        panic!("the stub script stayed busy");
    }

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn start_reports_progress_and_stop_ends_it_cleanly() {
        let dir = TempDir::new("capture-ctl");
        let stub = stub_ffmpeg(&dir.0);
        let mut c = CaptureController::new();
        start_stub(&mut c, &stub, &dir.0);
        assert!(c.is_running());
        assert!(
            c.start_with(&stub, &settings_in(&dir.0), &dir.0.join("logs"), "06:00:00")
                .is_err()
        );

        assert!(
            wait_until(|| {
                c.poll();
                c.stats.lock().unwrap().frame == 12
            }),
            "progress never appeared"
        );
        assert_eq!(c.stats.lock().unwrap().time, "00:00:00.40");

        c.stop();
        assert!(wait_until(|| {
            c.poll();
            !c.is_running()
        }));
        let log = fs::read_dir(dir.0.join("logs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(fs::read_to_string(log).unwrap().contains("interrupted"));
    }

    #[test]
    fn the_stop_timer_ends_the_capture() {
        let dir = TempDir::new("capture-timer");
        let stub = stub_ffmpeg(&dir.0);
        let mut c = CaptureController::new();
        start_stub(&mut c, &stub, &dir.0);
        c.arm_stop_timer(1);
        assert!(
            wait_until(|| {
                c.poll();
                !c.is_running()
            }),
            "the timer did not stop the capture"
        );
    }

    #[test]
    fn a_cancelled_timer_does_not_stop_the_capture() {
        let dir = TempDir::new("capture-cancel");
        let stub = stub_ffmpeg(&dir.0);
        let mut c = CaptureController::new();
        start_stub(&mut c, &stub, &dir.0);
        c.arm_stop_timer(1);
        c.cancel_stop_timer();
        thread::sleep(Duration::from_millis(1700));
        c.poll();
        assert!(c.is_running(), "a cancelled timer must not fire");
        c.stop();
        wait_until(|| {
            c.poll();
            !c.is_running()
        });
    }

    #[test]
    fn re_arming_replaces_the_previous_deadline() {
        let dir = TempDir::new("capture-rearm");
        let stub = stub_ffmpeg(&dir.0);
        let mut c = CaptureController::new();
        start_stub(&mut c, &stub, &dir.0);
        c.arm_stop_timer(1);
        c.arm_stop_timer(60); // extended: the 1 s deadline must not fire
        thread::sleep(Duration::from_millis(1700));
        c.poll();
        assert!(
            c.is_running(),
            "the first deadline fired after being replaced"
        );
        c.stop();
        wait_until(|| {
            c.poll();
            !c.is_running()
        });
    }

    #[test]
    fn a_capture_that_exits_on_its_own_is_reaped() {
        let dir = TempDir::new("capture-exit");
        let p = dir.0.join("short-stub");
        fs::write(&p, "#!/bin/sh\nsleep 0.2\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        let mut c = CaptureController::new();
        start_stub(&mut c, &p, &dir.0);
        assert!(wait_until(|| {
            c.poll();
            !c.is_running()
        }));
        assert_eq!(c.elapsed_str(), "--:--:--");
        // Idle again: a new capture can start.
        start_stub(&mut c, &p, &dir.0);
    }

    /// `text` with each listed `KEY=...` line replaced by `KEY="value"`.
    fn neutralize_env(text: &str, replace: &[(&str, &str)]) -> String {
        text.lines()
            .map(|line| {
                let key = line.split_once('=').map(|(k, _)| k.trim());
                match replace.iter().find(|(k, _)| Some(*k) == key) {
                    Some((k, v)) => format!("{k}=\"{v}\""),
                    None => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Runs the real `vhs_capture_ffmpeg.sh` against each shipped slot's `capture.env` with a stub
    /// ffmpeg that records its arguments, `HOME` pointed at a temp directory, and checks the Rust
    /// arguments are identical. Needs the sibling vhs-cli checkout, so it is not run by default.
    #[test]
    #[ignore]
    fn matches_the_bash_script() {
        let base = cfg();
        let script = base.scripts_dir.join("vhs_capture_ffmpeg.sh");
        assert!(script.is_file(), "script missing: {}", script.display());

        for slot in ["archival", "viewer"] {
            let env_src = base
                .scripts_dir
                .join(format!("vhs-env/{slot}/ffmpeg/capture.env"));
            let home = TempDir::new(&format!("capture-golden-{slot}"));
            let slot_dir = home.0.join("Videos/ffmpeg-current");
            fs::create_dir_all(&slot_dir).unwrap();

            let stub = |name: &str, body: &str| {
                let p = home.0.join(name);
                fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
                fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
                p
            };
            let calls = home.0.join("calls.txt");
            let ffmpeg = stub(
                "ffmpeg-stub",
                &format!("printf '%s\\n' \"$@\" > \"{}\"", calls.display()),
            );
            let ffprobe = stub("ffprobe-stub", "exit 0");

            // The script sources capture.env AFTER reading the environment, and the shipped file sets
            // VHS_FFMPEG_BIN=/usr/bin/ffmpeg, which would silently replace the stub with the real
            // ffmpeg and start a real capture from the USB grabber (this test once did exactly that
            // for five minutes). So the temp copy points both binaries at the stubs and also points
            // the video and audio devices at nonexistent ones: even a real ffmpeg could not capture.
            let original = fs::read_to_string(&env_src).unwrap();
            let neutral = neutralize_env(
                &original,
                &[
                    ("VHS_FFMPEG_BIN", &ffmpeg.to_string_lossy()),
                    ("VHS_FFPROBE_BIN", &ffprobe.to_string_lossy()),
                    ("VHS_V4L2_DEV", "/dev/vhs-gui-test-no-such-video"),
                    ("VHS_ALSA_DEV", "hw:CARD=vhsguitestnone,DEV=0"),
                ],
            );
            fs::write(slot_dir.join("capture.env"), &neutral).unwrap();

            // The compiled-in defaults must equal the shipped archival slot, untouched.
            if slot == "archival" {
                let c = Config {
                    videos_dir: home.0.join("Videos"),
                    ..Config::default()
                };
                let mut from_file = CaptureSettings::defaults(&c);
                from_file.apply_env_text(&original, &home.0.to_string_lossy());
                assert_eq!(from_file, CaptureSettings::defaults(&c));
            }

            let mut child = Command::new("bash")
                .arg(&script)
                .env("HOME", &home.0)
                .env("MAX_CAPTURE_DURATION", "06:00:00")
                .env("VHS_PREVIEW", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            // Hard limit: with stubs the script takes well under a second.
            let started = Instant::now();
            let status = loop {
                if let Some(st) = child.try_wait().unwrap() {
                    break st;
                }
                if started.elapsed() > Duration::from_secs(30) {
                    sigint_group(child.id() as i32);
                    let _ = child.kill();
                    panic!("{slot}: the script did not finish in 30 s");
                }
                thread::sleep(Duration::from_millis(50));
            };
            assert!(status.success(), "{slot}: script failed");
            assert!(calls.is_file(), "{slot}: the stub ffmpeg was never called");
            let from_script: Vec<String> = fs::read_to_string(&calls)
                .unwrap()
                .lines()
                .map(String::from)
                .collect();

            // Rust side: defaults for this HOME, then the slot's capture.env over them.
            let c = Config {
                videos_dir: home.0.join("Videos"),
                ..Config::default()
            };
            let mut settings = CaptureSettings::defaults(&c);
            settings.apply_env_text(&neutral, &home.0.to_string_lossy());

            // The script's output file carries its own timestamp; take it from the arguments and
            // check separately that it has the shape the Rust side would produce.
            let out = PathBuf::from(
                from_script
                    .iter()
                    .find(|a| a.ends_with(".mkv"))
                    .unwrap_or_else(|| panic!("{slot}: no .mkv output in {from_script:?}")),
            );
            assert_eq!(out.parent().unwrap(), settings.outdir, "{slot}");
            let name = out.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.starts_with(&format!("{}_", settings.prefix))
                    && name.len() == settings.prefix.len() + 1 + 19 + 4,
                "{slot}: unexpected output name {name}"
            );

            let ours = strings(&ffmpeg_args(&settings, "06:00:00", &out, true).unwrap());
            assert_eq!(ours, from_script, "{slot} slot");
        }
    }
}
