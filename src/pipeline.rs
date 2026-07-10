use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Instant, SystemTime};

use nix::unistd::Pid;

pub struct PipelineJob {
    child: Option<Child>,
    /// The producer process (`vspipe`) in a two-process native pipe.  `child`
    /// holds the consumer (`ffmpeg`), whose stderr drives progress parsing.
    /// `None` for bash jobs and single-process native jobs.
    aux_child: Option<Child>,
    /// Process-group id to signal.  Bash jobs: `child`'s pid (unchanged
    /// behavior — `.process_group(0)` made child pid == pgid).  Native pipe
    /// jobs: the producer's pid (the group leader the consumer joined).
    /// Native single jobs: the one child's pid.
    pub(crate) pgid: Pid,
    /// Set once the job's primary process exits: `Some(true)` on a clean exit,
    /// `Some(false)` on a non-zero exit.  `None` while still running.
    pub exit_ok: Option<bool>,
    /// Human-readable label shown in the UI (e.g. "VDecimate seg001.mkv")
    pub label: String,
    /// True once the child has exited (success or failure).
    pub done: bool,
    /// Most recent `frame=` value from ffmpeg progress output.
    pub current_frame: u64,
    /// Input frame count (display only).  May differ from output count for
    /// frame-rate-changing ops (VDecimate, QTGMC).  Not used for progress %.
    pub total_frames: u64,
    /// Most recent `time=` parsed from ffmpeg progress, in seconds.
    pub current_time_secs: f64,
    /// Input duration in seconds (from ffprobe).  Used as the denominator for
    /// time-based progress, which is accurate even when output fps differs from input fps.
    pub total_duration_secs: f64,
    /// Path to the log file we created for this job's stderr.
    script_log: Option<PathBuf>,
    started_at: Instant,
    // --- Upscale-specific tracking ---
    /// True when this job was launched by `launch_upscale()`.
    pub is_upscale: bool,
    /// Path to the `segments/` directory inside the upscale work dir.
    pub segments_dir: Option<PathBuf>,
    /// Path to the `frames/` directory — extracted source frames for the active segment.
    pub frames_dir: Option<PathBuf>,
    /// Path to the `frames_up/` directory — Real-ESRGAN output frames for the active segment.
    pub frames_up_dir: Option<PathBuf>,
    /// Number of completed `seg_*.mp4` files counted in `segments_dir`.
    pub completed_segments: u64,
    /// Total expected segments: `ceil(total_duration_secs / 30s)`.
    pub total_segments: u64,
    /// Frames written to `frames_up/` in the current segment (Real-ESRGAN output).
    pub upscaled_frames: u64,
    /// Frames present in `frames/` for the current segment (extraction output).
    pub segment_frames: u64,
    /// True while the process group is SIGSTOP-paused.
    pub paused: bool,
    /// Expected output file path — used after job completion to verify the
    /// output was created before deleting the work directory.
    pub output_path: Option<PathBuf>,
    /// When `Some(n)`, send SIGINT as soon as `completed_segments > n`.
    stop_after_segment_at: Option<u64>,
}

/// Shared preamble for every spawn path: probes the input, builds the log
/// file, and computes the `~/bin`-shimmed PATH.  Returns everything each
/// constructor needs to build its own `Command`(s) and the base `Self`.
struct SpawnPrep {
    label_str: String,
    total_frames: u64,
    total_duration_secs: f64,
    log_path: PathBuf,
    log_file: fs::File,
    full_path: String,
}

fn prepare_spawn(
    label: impl Into<String>,
    input: &Path,
    log_dir: &Path,
) -> anyhow::Result<SpawnPrep> {
    // Probe input video info before spawning (fast: reads container header).
    let (total_frames, total_duration_secs) = probe_video_info(input);

    let label_str: String = label.into();

    // Create a log file for this job; redirect both stdout and stderr there.
    // Stdout captures script-level status/error messages; stderr captures
    // ffmpeg progress lines (frame=, time=) that we tail for the progress bar.
    let slug: String = label_str
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let log_path = log_dir.join(format!("{slug}_{ts}.log"));
    let log_file = fs::File::create(&log_path)?;

    // Ensure ~/bin is on PATH so user-installed tools (realesrgan-rocm, etc.)
    // are findable even when the GUI is launched from a desktop session that
    // doesn't run the user's full shell profile.
    let home = std::env::var("HOME").unwrap_or_default();
    let home_bin = format!("{home}/bin");
    let current_path = std::env::var("PATH").unwrap_or_default();
    let full_path = if current_path.split(':').any(|p| p == home_bin) {
        current_path
    } else {
        format!("{home_bin}:{current_path}")
    };

    Ok(SpawnPrep {
        label_str,
        total_frames,
        total_duration_secs,
        log_path,
        log_file,
        full_path,
    })
}

/// Base fields shared by every constructor, given the pieces that differ.
fn base_job(prep: SpawnPrep, child: Child, aux_child: Option<Child>, pgid: Pid) -> PipelineJob {
    PipelineJob {
        child: Some(child),
        aux_child,
        pgid,
        exit_ok: None,
        label: prep.label_str,
        done: false,
        current_frame: 0,
        total_frames: prep.total_frames,
        current_time_secs: 0.0,
        total_duration_secs: prep.total_duration_secs,
        script_log: Some(prep.log_path),
        started_at: Instant::now(),
        is_upscale: false,
        segments_dir: None,
        frames_dir: None,
        frames_up_dir: None,
        completed_segments: 0,
        total_segments: 0,
        upscaled_frames: 0,
        segment_frames: 0,
        output_path: None,
        paused: false,
        stop_after_segment_at: None,
    }
}

impl PipelineJob {
    /// Spawn a pipeline script.
    ///
    /// * `label`   – display name for the UI
    /// * `script`  – path to the bash script
    /// * `input`   – path passed as `$1` to the script
    /// * `envs`    – extra environment variables (e.g. `&[("NO_LAUNCH", "1")]`)
    /// * `log_dir` – `~/Videos/logs/` — where scripts write their own progress logs
    pub fn start(
        label: impl Into<String>,
        script: &Path,
        input: &Path,
        envs: &[(&str, &str)],
        extra_args: &[&str],
        log_dir: &Path,
    ) -> anyhow::Result<Self> {
        use std::os::unix::process::CommandExt as _;

        let prep = prepare_spawn(label, input, log_dir)?;
        // Clone handle so both stdout and stderr land in the same log file.
        // This captures script-level error messages (which often go to stdout)
        // as well as ffmpeg progress lines (which go to stderr).
        let log_file_out = prep.log_file.try_clone()?;

        let mut cmd = Command::new("bash");
        cmd.arg(script).arg(input);
        for a in extra_args {
            cmd.arg(a);
        }
        cmd.stdin(Stdio::null())
            // Both stdout and stderr go to the log so we see all script output.
            .stdout(Stdio::from(log_file_out))
            .stderr(Stdio::from(prep.log_file.try_clone()?))
            .env("PATH", &prep.full_path)
            .process_group(0); // own PGID so killpg() doesn't reach vhs-gui

        for (k, v) in envs {
            cmd.env(k, v);
        }

        let child = cmd.spawn()?;
        let pgid = Pid::from_raw(child.id() as i32);
        Ok(base_job(prep, child, None, pgid))
    }

    /// Spawn a native two-process pipe: `producer | consumer` (e.g. `vspipe |
    /// ffmpeg`). `producer`/`consumer` must already have their args/env set;
    /// this function wires stdio (piped stdout -> stdin), the shared process
    /// group, and log redirection. The consumer's stderr drives progress
    /// parsing (same as the bash path).
    pub fn start_native_pipe(
        label: impl Into<String>,
        input: &Path,
        mut producer: Command,
        mut consumer: Command,
        log_dir: &Path,
    ) -> anyhow::Result<Self> {
        use std::os::unix::process::CommandExt as _;

        let prep = prepare_spawn(label, input, log_dir)?;

        producer
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(prep.log_file.try_clone()?))
            .env("PATH", &prep.full_path)
            .process_group(0); // producer becomes its own group leader

        let mut producer_child = producer.spawn()?;
        let producer_pid = producer_child.id();
        let producer_stdout = producer_child
            .stdout
            .take()
            .expect("producer spawned with Stdio::piped()");

        consumer
            .stdin(Stdio::from(producer_stdout))
            .stdout(Stdio::from(prep.log_file.try_clone()?))
            .stderr(Stdio::from(prep.log_file.try_clone()?))
            .env("PATH", &prep.full_path)
            .process_group(producer_pid as i32); // join the producer's group

        let consumer_child = consumer.spawn()?;
        let pgid = Pid::from_raw(producer_pid as i32);
        Ok(base_job(prep, consumer_child, Some(producer_child), pgid))
    }

    /// Spawn a single native process (no pipe) — e.g. the one-shot `ffmpeg`
    /// call used by A/V sync correction.
    pub fn start_native_single(
        label: impl Into<String>,
        input: &Path,
        mut cmd: Command,
        log_dir: &Path,
    ) -> anyhow::Result<Self> {
        use std::os::unix::process::CommandExt as _;

        let prep = prepare_spawn(label, input, log_dir)?;

        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(prep.log_file.try_clone()?))
            .stderr(Stdio::from(prep.log_file.try_clone()?))
            .env("PATH", &prep.full_path)
            .process_group(0);

        let child = cmd.spawn()?;
        let pgid = Pid::from_raw(child.id() as i32);
        Ok(base_job(prep, child, None, pgid))
    }

    /// Non-blocking poll: check child exit and tail the script's own log for
    /// frame progress.
    pub fn poll(&mut self) {
        // Must update counts before the stop-after-segment check so we see
        // the freshly completed segment that triggered the threshold.
        self.update_frame_count();

        // Stop-after-segment: send SIGINT once the segment we were waiting on
        // has been written to disk (completed_segments advanced past the
        // snapshot taken when the user clicked the button).
        //
        // Exception: if all segments are already done the script is in the
        // final concat/mux phase — clearing the stop request lets it finish
        // and produce the output file rather than killing it mid-concat.
        if let Some(stop_at) = self.stop_after_segment_at {
            if self.total_segments > 0 && self.completed_segments >= self.total_segments {
                self.stop_after_segment_at = None; // let concat finish
            } else if self.completed_segments > stop_at {
                self.cancel();
                self.stop_after_segment_at = None;
            }
        }

        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_ok = Some(status.success());
                    self.child = None;
                    self.done = true;
                    self.reap_aux_child();
                }
                Err(_) => {
                    self.exit_ok = Some(false);
                    self.child = None;
                    self.done = true;
                    self.reap_aux_child();
                }
                Ok(None) => {}
            }
        } else {
            self.done = true;
        }
    }

    /// Kill (idempotent — a no-op if it already exited, e.g. from SIGPIPE when
    /// the consumer closed its stdin) and reap the producer side of a native
    /// pipe job, so it never becomes a zombie.
    fn reap_aux_child(&mut self) {
        if let Some(mut aux) = self.aux_child.take() {
            let _ = aux.kill();
            let _ = aux.wait();
        }
    }

    /// Fractional progress in `0.0..=1.0`.  Returns `None` when total duration is unknown.
    /// Uses time-based progress (current_time / total_duration) which is accurate
    /// regardless of frame-rate changes (VDecimate, QTGMC, etc.).
    pub fn progress(&self) -> Option<f32> {
        if self.total_duration_secs > 0.0 {
            Some((self.current_time_secs / self.total_duration_secs).min(1.0) as f32)
        } else {
            None
        }
    }

    pub fn elapsed_str(&self) -> String {
        let d = self.started_at.elapsed();
        let h = d.as_secs() / 3600;
        let m = (d.as_secs() % 3600) / 60;
        let s = d.as_secs() % 60;
        if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        }
    }

    /// Configure upscale-specific dual-progress tracking.
    /// Call immediately after `start()`, before storing the job.
    ///
    /// * `segments_dir` – `WORK_ROOT/<stem>/segments/`; `frames/` and
    ///   `frames_up/` are derived as siblings.
    /// * `output_path` – expected final output file; checked after the job
    ///   completes to decide whether to delete the work dir.
    pub fn with_upscale_tracking(
        mut self,
        segments_dir: PathBuf,
        output_path: PathBuf,
        segment_secs: u32,
    ) -> Self {
        self.is_upscale = true;
        if let Some(work_dir) = segments_dir.parent() {
            self.frames_dir = Some(work_dir.join("frames"));
            self.frames_up_dir = Some(work_dir.join("frames_up"));
        }
        self.segments_dir = Some(segments_dir);
        self.output_path = Some(output_path);
        self.total_segments = if self.total_duration_secs > 0.0 && segment_secs > 0 {
            (self.total_duration_secs / segment_secs as f64).ceil() as u64
        } else {
            0
        };
        self
    }

    /// Progress through the Real-ESRGAN upscaling step of the current segment:
    /// `upscaled_frames / segment_frames`.
    /// Returns `None` for non-upscale jobs or when no frames have been extracted yet.
    pub fn segment_progress(&self) -> Option<f32> {
        if !self.is_upscale || self.segment_frames == 0 {
            return None;
        }
        Some((self.upscaled_frames as f32 / self.segment_frames as f32).min(1.0))
    }

    /// Overall upscale progress: completed segments / total segments.
    /// Returns `None` for non-upscale jobs or when total is unknown.
    pub fn total_progress(&self) -> Option<f32> {
        if !self.is_upscale || self.total_segments == 0 {
            return None;
        }
        Some((self.completed_segments as f32 / self.total_segments as f32).min(1.0))
    }

    /// Send SIGINT to the job's process group.
    /// Safe because every constructor sets `pgid` to the actual group leader
    /// (child pid for bash/single jobs, producer pid for native pipe jobs).
    pub fn cancel(&self) {
        if self.child.is_some() {
            use nix::sys::signal::{Signal, killpg};
            let _ = killpg(self.pgid, Signal::SIGINT);
        }
    }

    /// Toggle SIGSTOP / SIGCONT on the entire process group.
    /// All processes in the group (bash's children, or the native
    /// producer+consumer pair) are paused / resumed together.
    pub fn toggle_pause(&mut self) {
        use nix::sys::signal::{Signal, killpg};
        if self.child.is_some() {
            if self.paused {
                let _ = killpg(self.pgid, Signal::SIGCONT);
                self.paused = false;
            } else {
                let _ = killpg(self.pgid, Signal::SIGSTOP);
                self.paused = true;
            }
        }
    }

    /// Request a clean stop: send SIGINT once the in-progress segment finishes.
    /// The completed checkpoint files are preserved for resuming later.
    pub fn request_stop_after_segment(&mut self) {
        self.stop_after_segment_at = Some(self.completed_segments);
    }

    /// True while waiting for the current segment to finish before stopping.
    pub fn stopping_after_segment(&self) -> bool {
        self.stop_after_segment_at.is_some()
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    /// Tail our stderr-redirect log file for ffmpeg progress output.
    /// Parses both `frame=` (for the frame counter display) and `time=HH:MM:SS.xx`
    /// (for accurate time-based progress that works regardless of fps changes).
    fn update_frame_count(&mut self) {
        let Some(ref log) = self.script_log else {
            return;
        };

        let Ok(file) = fs::File::open(log) else {
            return;
        };
        let reader = BufReader::new(file);

        // ffmpeg writes progress as `\r`-delimited runs within a single `\n`-line
        // (or as plain `\n`-lines when not a tty).  Split on both to be safe.
        let mut last_frame: Option<u64> = None;
        let mut last_time: Option<f64> = None;
        for raw_line in reader.lines().map_while(|l| l.ok()) {
            for segment in raw_line.split('\r') {
                if let Some(f) = parse_field(segment, "frame=")
                    && let Ok(n) = f.parse::<u64>()
                {
                    last_frame = Some(n);
                }
                if let Some(t) = parse_field(segment, "time=")
                    && let Some(secs) = parse_hms(t)
                {
                    last_time = Some(secs);
                }
            }
        }
        if let Some(f) = last_frame {
            self.current_frame = f;
        }
        if let Some(t) = last_time {
            self.current_time_secs = t;
        }

        // For upscale jobs: count completed segments and per-segment frame progress.
        if self.is_upscale
            && let Some(ref seg_dir) = self.segments_dir
            && let Ok(rd) = fs::read_dir(seg_dir)
        {
            self.completed_segments = rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension().and_then(|s| s.to_str()) == Some("mp4")
                        && e.metadata().map(|m| m.len() > 0).unwrap_or(false)
                })
                .count() as u64;
        }
        if self.is_upscale {
            let count_dir = |dir: &Option<PathBuf>| -> u64 {
                dir.as_ref()
                    .and_then(|d| fs::read_dir(d).ok())
                    .map(|rd| rd.filter_map(|e| e.ok()).count() as u64)
                    .unwrap_or(0)
            };
            self.segment_frames = count_dir(&self.frames_dir);
            self.upscaled_frames = count_dir(&self.frames_up_dir);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let idx = line.find(key)?;
    let rest = &line[idx + key.len()..];
    Some(
        rest.split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/'),
    )
}

/// Probe input video: returns (total_frames, duration_secs).
/// Fast: reads only the container header/index.
/// Returns (0, 0.0) on failure.
///
/// Uses format-level duration (not stream-level) because FFV1 MKV files produced
/// by QTGMC/VDecimate often store N/A for stream duration — the real value is
/// in the container (format) section only.
fn probe_video_info(path: &Path) -> (u64, f64) {
    let out = Command::new("/usr/bin/ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            // fps from stream, duration from format (container) — one value per line
            "-show_entries",
            "stream=r_frame_rate:format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output();

    let Ok(out) = out else { return (0, 0.0) };
    let s = String::from_utf8_lossy(&out.stdout);
    // Output is two lines: fps fraction then duration in seconds
    //   e.g. "30000/1001\n3672.100000\n"
    let mut lines = s.lines();
    let fps_str = lines.next().unwrap_or("0/1");
    let duration_s: f64 = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0.0);

    let mut fps_parts = fps_str.split('/');
    let num: f64 = fps_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let den: f64 = fps_parts.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    if den == 0.0 || num == 0.0 || duration_s == 0.0 {
        return (0, 0.0);
    }
    let frames = (duration_s * num / den).round() as u64;
    (frames, duration_s)
}

/// Parse ffmpeg's `time=HH:MM:SS.xx` field into seconds.
fn parse_hms(s: &str) -> Option<f64> {
    // Format: "HH:MM:SS.xx" — ignore N/A
    if s.starts_with('N') {
        return None;
    }
    let mut parts = s.splitn(3, ':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}
