//! The live monitor's video source.
//!
//! mpv used to open the V4L2 grabber itself. Closing it (every time a capture starts, or on Stop
//! Monitor) crashed the GUI: libavdevice's buffer-release callback ran after the device was closed,
//! on mpv's demuxer thread (core dumps on 10-30, 11-02 and four times on 11-03, always the same
//! stack, none of it vhs-gui code). So mpv no longer touches `/dev/video`: a small ffmpeg process
//! reads the grabber and sends the same low-latency MPEG-TS preview a capture sends, and mpv plays
//! that over UDP.
//!
//! Unlike a capture, the feed must NOT outlive the GUI: it holds the grabber and records nothing.
//! `PR_SET_PDEATHSIG` makes the kernel terminate it if the GUI dies, so a crash cannot leave the
//! device held. (The signal fires when the *thread* that spawned it exits; the feed is started from
//! the UI thread, which lives as long as the process.)

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::capture::CaptureSettings;
use crate::config::Config;
use crate::tools::{self, Tool};

/// ffmpeg arguments for the feed: the capture's video input, then the capture's preview encode to
/// the preview URL. No audio, nothing recorded.
pub fn feed_args(s: &CaptureSettings) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut add = |items: &[&str]| a.extend(items.iter().map(OsString::from));
    add(&["-hide_banner", "-nostdin", "-loglevel", "warning"]);
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
    let vf = format!("scale={}", s.preview_scale);
    add(&["-map", "0:v:0", "-vf", &vf]);
    add(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
    ]);
    add(&[
        "-g",
        "15",
        "-pix_fmt",
        "yuv420p",
        "-an",
        "-f",
        "mpegts",
        &s.preview_url,
    ]);
    a
}

pub struct MonitorFeed {
    child: Option<Child>,
    log_path: Option<PathBuf>,
}

impl MonitorFeed {
    pub fn new() -> Self {
        Self {
            child: None,
            log_path: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    pub fn start(&mut self, cfg: &Config) -> anyhow::Result<()> {
        let ffmpeg = tools::require(Tool::Ffmpeg)?;
        let settings = CaptureSettings::load(cfg);
        self.start_with(&ffmpeg, &settings, &cfg.log_dir())
    }

    /// `start` with the program and settings spelled out (tests substitute a stub for ffmpeg).
    fn start_with(
        &mut self,
        program: &Path,
        settings: &CaptureSettings,
        log_dir: &Path,
    ) -> anyhow::Result<()> {
        if self.poll() {
            anyhow::bail!("the previous monitor is still shutting down; try again in a moment");
        }
        fs::create_dir_all(log_dir)?;
        let log = log_dir.join("monitor.ffmpeg.log");
        let log_file = fs::File::create(&log)?;

        let mut cmd = Command::new(program);
        cmd.args(feed_args(settings))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log_file))
            .process_group(0);
        // SAFETY: the closure only calls `prctl`, which is async-signal-safe, between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGTERM as libc::c_ulong,
                    0,
                    0,
                    0,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        self.child = Some(cmd.spawn()?);
        self.log_path = Some(log);
        Ok(())
    }

    /// Reap the feed if it has exited. Returns whether it is still running.
    pub fn poll(&mut self) -> bool {
        if let Some(child) = self.child.as_mut()
            && !matches!(child.try_wait(), Ok(None))
        {
            self.child = None;
        }
        self.child.is_some()
    }

    fn signal_group(&self, sig: nix::sys::signal::Signal) {
        if let Some(child) = &self.child {
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(child.id() as i32), sig);
        }
    }

    /// Ask the feed to finish (SIGINT). It exits on its own shortly; `poll` reaps it.
    pub fn stop(&self) {
        self.signal_group(nix::sys::signal::Signal::SIGINT);
    }

    /// Kill the feed outright, for one that ignores `stop`.
    pub fn kill(&mut self) {
        self.signal_group(nix::sys::signal::Signal::SIGKILL);
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    /// The last line ffmpeg wrote to its log, to say why a feed ended on its own.
    pub fn last_log_line(&self) -> Option<String> {
        let mut f = fs::File::open(self.log_path.as_ref()?).ok()?;
        let len = f.metadata().ok()?.len();
        f.seek(SeekFrom::Start(len.saturating_sub(2048))).ok()?;
        let mut buf = String::new();
        f.read_to_string(&mut buf).ok()?;
        buf.lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_owned)
    }
}

impl Drop for MonitorFeed {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::Duration;

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

    fn settings() -> CaptureSettings {
        CaptureSettings::defaults(&Config::default())
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Start `feed` on a freshly written script. Parallel tests can fork while the script is still
    /// open for writing, so exec may briefly fail with "Text file busy"; retry that one error.
    fn start(feed: &mut MonitorFeed, prog: &Path, dir: &Path) {
        for _ in 0..100 {
            match feed.start_with(prog, &settings(), &dir.join("logs")) {
                Ok(()) => return,
                Err(e) if e.to_string().contains("Text file busy") => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("start_with: {e}"),
            }
        }
        panic!("the script stayed busy");
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
    fn feed_arguments_are_the_captures_input_and_preview() {
        let args: Vec<String> = feed_args(&settings())
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let want = "-hide_banner -nostdin -loglevel warning \
            -f v4l2 -use_wallclock_as_timestamps 1 -thread_queue_size 1024 -input_format yuyv422 \
            -video_size 720x480 -framerate 30 \
            -i /dev/v4l/by-id/usb-MACROSIL_AV_TO_USB2.0-video-index0 \
            -map 0:v:0 -vf scale=480:360 -c:v libx264 -preset ultrafast -tune zerolatency \
            -g 15 -pix_fmt yuv420p -an -f mpegts udp://127.0.0.1:23000?pkt_size=1316";
        assert_eq!(args.join(" "), want);
        // A feed records nothing.
        assert!(!args.iter().any(|a| a.ends_with(".mkv")));
    }

    #[test]
    fn the_feed_can_be_stopped_and_started_again() {
        let dir = TempDir::new("feed-stop");
        let stub = script(
            &dir.0,
            "ffmpeg-stub",
            "#!/bin/sh\ntrap 'exit 0' INT\nwhile :; do sleep 0.05; done\n",
        );
        let mut feed = MonitorFeed::new();
        start(&mut feed, &stub, &dir.0);
        assert!(feed.is_running());
        assert!(feed.poll());

        feed.stop();
        assert!(wait_until(|| !feed.poll()), "the feed did not stop");
        assert!(!feed.is_running());

        start(&mut feed, &stub, &dir.0); // a stopped feed can start again
        assert!(feed.poll());
    }

    #[test]
    fn starting_while_one_is_still_running_is_refused() {
        let dir = TempDir::new("feed-twice");
        let stub = script(
            &dir.0,
            "ffmpeg-stub",
            "#!/bin/sh\nwhile :; do sleep 0.05; done\n",
        );
        let mut feed = MonitorFeed::new();
        start(&mut feed, &stub, &dir.0);
        let err = feed
            .start_with(&stub, &settings(), &dir.0.join("logs"))
            .unwrap_err();
        assert!(err.to_string().contains("shutting down"), "{err}");
    }

    #[test]
    fn a_feed_that_ignores_stop_can_be_killed() {
        let dir = TempDir::new("feed-kill");
        let stub = script(
            &dir.0,
            "ffmpeg-stub",
            "#!/bin/sh\ntrap '' INT\n: > \"$0.ready\"\nwhile :; do sleep 0.05; done\n",
        );
        let mut feed = MonitorFeed::new();
        start(&mut feed, &stub, &dir.0);
        // Signal only once the stub has really installed its trap, or SIGINT would just kill it.
        let ready = dir.0.join("ffmpeg-stub.ready");
        assert!(
            wait_until(|| ready.is_file()),
            "the stub never became ready"
        );
        feed.stop();
        thread::sleep(Duration::from_millis(300));
        assert!(
            feed.poll(),
            "the stub ignores SIGINT, so it must still be running"
        );
        feed.kill();
        assert!(!feed.is_running());
    }

    #[test]
    fn a_feed_that_exits_on_its_own_reports_why() {
        let dir = TempDir::new("feed-exit");
        let stub = script(
            &dir.0,
            "ffmpeg-stub",
            "#!/bin/sh\necho 'first line' >&2\necho 'Device or resource busy' >&2\nexit 1\n",
        );
        let mut feed = MonitorFeed::new();
        start(&mut feed, &stub, &dir.0);
        assert!(wait_until(|| !feed.poll()));
        assert_eq!(
            feed.last_log_line().as_deref(),
            Some("Device or resource busy")
        );
    }

    /// The feed asks the kernel to terminate it when the GUI dies. The child reads its own
    /// parent-death signal (PR_GET_PDEATHSIG, which survives exec) and writes it down.
    #[test]
    fn the_feed_dies_with_the_gui() {
        let dir = TempDir::new("feed-pdeath");
        let stub = script(
            &dir.0,
            "ffmpeg-stub",
            "#!/usr/bin/env python3\n\
             import ctypes, sys, time\n\
             v = ctypes.c_int(0)\n\
             ctypes.CDLL(None).prctl(2, ctypes.byref(v), 0, 0, 0)\n\
             open(sys.argv[0] + '.pdeathsig', 'w').write(str(v.value))\n\
             time.sleep(30)\n",
        );
        let mut feed = MonitorFeed::new();
        start(&mut feed, &stub, &dir.0);
        let out = dir.0.join("ffmpeg-stub.pdeathsig");
        assert!(wait_until(|| out.is_file()), "the stub never reported");
        thread::sleep(Duration::from_millis(100));
        let sig: i32 = fs::read_to_string(&out).unwrap().trim().parse().unwrap();
        assert_eq!(
            sig,
            libc::SIGTERM,
            "the feed must be set to die with its parent"
        );
    }

    /// Real ffmpeg and the real grabber: the feed sends an MPEG-TS stream to the preview port, stops
    /// on request, and afterwards the grabber is free for another process to open. Needs the USB
    /// grabber and UDP port 23000 (so the GUI must not be running); not run by default.
    #[test]
    #[ignore]
    fn the_feed_streams_from_the_grabber_and_lets_go_of_it() {
        use std::net::UdpSocket;
        use std::time::Instant;

        let cfg = Config::default();
        let sock =
            UdpSocket::bind("127.0.0.1:23000").expect("port 23000 is busy: is the GUI open?");
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

        let mut feed = MonitorFeed::new();
        feed.start(&cfg).unwrap();
        let mut buf = [0u8; 2048];
        let (mut packets, mut bad_sync) = (0, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while packets < 100 && Instant::now() < deadline {
            match sock.recv(&mut buf) {
                Ok(n) if n > 0 => {
                    packets += 1;
                    if buf[0] != 0x47 {
                        bad_sync += 1; // every MPEG-TS packet starts with 0x47
                    }
                }
                _ => break,
            }
        }
        assert!(
            packets >= 100,
            "only {packets} packets arrived; ffmpeg said: {:?}",
            feed.last_log_line()
        );
        assert_eq!(bad_sync, 0, "the stream is not MPEG-TS");
        assert!(feed.poll(), "the feed must still be running");

        feed.stop();
        assert!(wait_until(|| !feed.poll()), "the feed did not stop");

        // The grabber is free: ffmpeg can open it and read a few frames.
        let settings = CaptureSettings::load(&cfg);
        let probe = tools::command(Tool::Ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(["-f", "v4l2", "-input_format", &settings.input_fmt])
            .args(["-video_size", &settings.size, "-framerate", &settings.fps])
            .args([
                "-i",
                &settings.v4l2_dev,
                "-frames:v",
                "5",
                "-f",
                "null",
                "-",
            ])
            .status()
            .unwrap();
        assert!(probe.success(), "the grabber was not released by the feed");
    }
}
