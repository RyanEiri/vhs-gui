use std::time::{Duration, Instant};

use crate::capture::{self, CaptureController, OrphanCapture};
use crate::config::Config;
use crate::monitor_feed::MonitorFeed;
use crate::mpv_view::{MpvView, Source};
use crate::v4l2::V4l2Controls;

/// How often an idle panel looks for a capture it did not start.
const ORPHAN_SCAN_EVERY: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq)]
pub enum CaptureState {
    Idle,
    /// GUI owns V4L2 device for live monitor.
    Monitoring,
    /// Waiting for mpv to release the V4L2 fd before spawning ffmpeg.
    Releasing,
    /// ffmpeg capture subprocess is running.
    Capturing,
}

pub struct MonitorPanel {
    pub capture: CaptureController,
    pub state: CaptureState,
    /// Capture ffmpegs already running that this process did not start (for example one that
    /// outlived a GUI crash). They hold the grabber, so Monitor and Start Capture are replaced by
    /// a banner offering to re-attach or stop them.
    orphans: Vec<OrphanCapture>,
    orphan_scan_due: Instant,
    /// The ffmpeg process that feeds the live monitor. mpv never opens the grabber itself (see
    /// `monitor_feed`).
    pub feed: MonitorFeed,
    feed_reopen_at: Option<Instant>,
    releasing_at: Option<Instant>,
    /// True once the UDP preview stream has been opened in mpv.
    pub preview_opened: bool,
    capture_last_reopen_at: Option<Instant>,
    /// Editable "Cap:" max-duration field (shared with capture script env).
    pub max_duration: String,
    /// Editable field for the mid-capture stop timer.
    pub capture_stop_input: String,
    /// GUI-side deadline for the countdown display (OS timer thread is authoritative).
    pub capture_stop_at: Option<Instant>,
    /// V4L2 hardware controls (brightness, contrast, saturation, hue, gamma).
    pub v4l2: V4l2Controls,
    /// Whether the "Input" settings side panel is open.
    pub input_panel_open: bool,
}

impl MonitorPanel {
    pub fn new(cfg: &Config) -> Self {
        Self {
            capture: CaptureController::new(),
            state: CaptureState::Idle,
            orphans: capture::find_orphans(),
            orphan_scan_due: Instant::now() + ORPHAN_SCAN_EVERY,
            feed: MonitorFeed::new(),
            feed_reopen_at: None,
            releasing_at: None,
            preview_opened: false,
            capture_last_reopen_at: None,
            max_duration: cfg.max_capture_duration.clone(),
            capture_stop_input: String::new(),
            capture_stop_at: None,
            v4l2: V4l2Controls::new(&cfg.v4l2_device),
            input_panel_open: true,
        }
    }

    /// Draw the V4L2 hardware-control sliders inside a caller-supplied panel.
    /// Returns true if any control value changed this frame (triggers a settings save).
    pub fn show_input_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let (changed, close) = self.v4l2.show_panel(ui, true);
        if close {
            self.input_panel_open = false;
        }
        changed
    }

    /// Renders the capture-state portion of the toolbar.
    /// Returns true when the library should be refreshed (capture stopped via button).
    pub fn toolbar_section(
        &mut self,
        ui: &mut egui::Ui,
        mpv: &mut MpvView,
        cfg: &Config,
        status: &mut String,
    ) -> bool {
        let mut needs_refresh = false;
        match self.state {
            CaptureState::Idle if !self.orphans.is_empty() => {
                needs_refresh |= self.orphan_banner(ui, cfg, status);
            }
            CaptureState::Idle => {
                if ui.button("Monitor").clicked() {
                    match self.start_monitor(mpv, cfg) {
                        Ok(()) => *status = "Monitoring live signal".into(),
                        Err(e) => *status = format!("Monitor failed to start: {e}"),
                    }
                }
                if ui.button("Start Capture").clicked() {
                    self.begin_capture(cfg, status);
                }
            }
            CaptureState::Monitoring => {
                ui.label(egui::RichText::new("● MONITOR").color(egui::Color32::GREEN));
                if ui.button("Start Capture").clicked() {
                    self.begin_capture(cfg, status);
                }
                if ui.button("Stop Monitor").clicked() {
                    self.feed.stop();
                    mpv.stop();
                    self.state = CaptureState::Idle;
                    *status = "Idle".into();
                }
            }
            CaptureState::Releasing => {
                ui.label(egui::RichText::new("Releasing device…").italics());
            }
            CaptureState::Capturing => {
                ui.label(egui::RichText::new("● CAPTURE").color(egui::Color32::RED));
                let stats = self.capture.stats.lock().unwrap().clone();
                ui.label(format!(
                    "  {}  frame {}  {}  {}",
                    self.capture.elapsed_str(),
                    stats.frame,
                    stats.time,
                    stats.bitrate,
                ));
                if ui.button("Stop Capture").clicked() {
                    needs_refresh = self.do_stop_capture(status);
                }
                ui.separator();
                ui.label("Stop after:");
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.capture_stop_input)
                        .desired_width(70.0)
                        .hint_text("HH:MM:SS"),
                );
                let set_clicked = ui.button("Set").clicked();
                if (set_clicked
                    || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                    && let Some(secs) = parse_duration_secs(&self.capture_stop_input)
                {
                    self.capture.arm_stop_timer(secs);
                    self.capture_stop_at = Some(Instant::now() + Duration::from_secs(secs));
                    *status = format!("Stopping in {}", fmt_secs(secs));
                }
                if let Some(deadline) = self.capture_stop_at {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    ui.label(
                        egui::RichText::new(format!("⏱ {}", fmt_secs(remaining.as_secs())))
                            .color(egui::Color32::YELLOW)
                            .small(),
                    );
                }
            }
        }
        needs_refresh
    }

    /// Called every frame from `App::update`. Handles:
    /// - `capture.poll()` (reap child, tail log, update stats)
    /// - Releasing → Capturing transition
    /// - UDP preview open + insurance reopen
    /// - GUI-side timed stop (backup; OS timer thread is authoritative)
    /// - Natural capture end detection
    ///
    /// Returns true when the library should be refreshed.
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        mpv: &mut MpvView,
        cfg: &Config,
        status: &mut String,
    ) -> bool {
        self.capture.poll();
        self.feed.poll();

        // While idle, notice a capture that outlived its GUI (or was started elsewhere).
        if self.state == CaptureState::Idle {
            if Instant::now() >= self.orphan_scan_due {
                self.orphans = capture::find_orphans();
                self.orphan_scan_due = Instant::now() + ORPHAN_SCAN_EVERY;
            }
            ctx.request_repaint_after(ORPHAN_SCAN_EVERY);
        }

        // Monitoring: the feed ended on its own (device unplugged, ffmpeg error), or mpv has not
        // received the stream yet and needs another open.
        if self.state == CaptureState::Monitoring {
            if !self.feed.is_running() {
                let why = self.feed.last_log_line().unwrap_or_default();
                mpv.stop();
                self.state = CaptureState::Idle;
                *status = format!("Monitor stopped: {why}");
            } else {
                if mpv.state.idle
                    && self
                        .feed_reopen_at
                        .map(|t| t.elapsed() > Duration::from_secs(2))
                        .unwrap_or(true)
                {
                    mpv.open(&Source::Udp(capture::PREVIEW_URL.into()));
                    self.feed_reopen_at = Some(Instant::now());
                }
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }

        // Releasing → Capturing once the monitor feed has actually exited and let go of the
        // grabber. (mpv's own idle state says nothing about that.)
        if self.state == CaptureState::Releasing {
            let waited = self.releasing_at.map(|t| t.elapsed()).unwrap_or_default();
            if !self.feed.is_running() {
                self.releasing_at = None;
                self.do_start_capture(cfg, status);
            } else {
                if waited > Duration::from_secs(3) {
                    self.feed.kill();
                }
                ctx.request_repaint();
            }
        }

        if self.state != CaptureState::Capturing {
            return false;
        }

        ctx.request_repaint_after(Duration::from_secs(1));

        // Open the UDP preview once ffmpeg confirms it's running.
        if !self.preview_opened && self.capture.is_running() {
            mpv.open(&Source::Udp(crate::capture::PREVIEW_URL.into()));
            self.preview_opened = true;
            self.capture_last_reopen_at = Some(Instant::now());
            *status = "Capturing… (previewing)".into();
        }

        // Insurance reopen: if mpv went idle before ffmpeg started sending.
        if self.preview_opened && mpv.state.idle && self.capture.is_running() {
            let can_reopen = self
                .capture_last_reopen_at
                .map(|t| t.elapsed() > Duration::from_secs(2))
                .unwrap_or(true);
            if can_reopen {
                mpv.open(&Source::Udp(crate::capture::PREVIEW_URL.into()));
                self.capture_last_reopen_at = Some(Instant::now());
            }
        }

        // GUI-side timed stop (backup for the OS timer thread).
        if self
            .capture_stop_at
            .map(|d| Instant::now() >= d)
            .unwrap_or(false)
        {
            // Stop it and let the natural-end branch below resume the monitor once ffmpeg has
            // actually exited and released the grabber.
            self.capture.stop();
            self.capture_stop_at = None;
            *status = "Stopping capture (timer)…".into();
        }

        // Natural end: ffmpeg exited on its own (hit -t cap, or normal finish).
        if !self.capture.is_running() {
            *status = match self.end_capture_to_monitoring(mpv, cfg) {
                Ok(()) => "Capture ended — monitoring live signal".into(),
                Err(e) => format!("Capture ended — monitor failed to start: {e}"),
            };
            return true;
        }

        false
    }

    /// Start the monitor: the feed ffmpeg reads the grabber and mpv plays its UDP preview.
    fn start_monitor(&mut self, mpv: &mut MpvView, cfg: &Config) -> anyhow::Result<()> {
        self.feed.start(cfg)?;
        mpv.open(&Source::Udp(capture::PREVIEW_URL.into()));
        self.feed_reopen_at = Some(Instant::now());
        self.state = CaptureState::Monitoring;
        Ok(())
    }

    /// Shared cleanup after a timer-driven capture end (natural `-t` safety
    /// cap, or the GUI "Stop after" deadline) — the archival file is done,
    /// but the user likely still wants the live feed rather than a frozen
    /// last frame, so hand the grabber straight back to the monitor
    /// instead of dropping to Idle. Manual "Stop Capture" is unaffected —
    /// it still goes to Idle via `do_stop_capture`.
    fn end_capture_to_monitoring(&mut self, mpv: &mut MpvView, cfg: &Config) -> anyhow::Result<()> {
        self.preview_opened = false;
        self.capture_last_reopen_at = None;
        self.capture_stop_at = None;
        self.capture_stop_input.clear();
        match self.start_monitor(mpv, cfg) {
            Ok(()) => Ok(()),
            Err(e) => {
                mpv.stop();
                self.state = CaptureState::Idle;
                Err(e)
            }
        }
    }

    fn begin_capture(&mut self, cfg: &Config, status: &mut String) {
        if self.state == CaptureState::Monitoring {
            // Stop the feed and wait (in `poll`) until it has exited and let go of the grabber;
            // mpv keeps playing the last of the stream and never touches the device.
            self.feed.stop();
            self.state = CaptureState::Releasing;
            self.releasing_at = Some(Instant::now());
            *status = "Releasing device…".into();
        } else {
            self.do_start_capture(cfg, status);
        }
    }

    /// The "a capture is already running" banner. Returns true when the library should refresh.
    fn orphan_banner(&mut self, ui: &mut egui::Ui, cfg: &Config, status: &mut String) -> bool {
        let Some(orphan) = self.orphans.first().cloned() else {
            return false;
        };
        ui.label(
            egui::RichText::new(format!(
                "⚠ A capture is already running: {} (for {})",
                orphan.name(),
                fmt_secs(orphan.age_secs)
            ))
            .color(egui::Color32::YELLOW),
        );
        if orphan.is_group_leader()
            && ui
                .button("Reattach")
                .on_hover_text("Show its progress, stop it from here, set a stop time")
                .clicked()
        {
            match self.capture.adopt(&orphan, &cfg.log_dir()) {
                Ok(()) => {
                    self.orphans.clear();
                    self.state = CaptureState::Capturing;
                    self.preview_opened = false;
                    self.capture_last_reopen_at = Some(Instant::now());
                    self.capture_stop_input.clear();
                    self.capture_stop_at = None;
                    *status = "Reattached to the running capture".into();
                }
                Err(e) => *status = format!("Cannot reattach: {e}"),
            }
        }
        if ui
            .button("Stop it")
            .on_hover_text("Interrupt ffmpeg so the file is finalized")
            .clicked()
        {
            for o in &self.orphans {
                o.stop();
            }
            self.orphan_scan_due = Instant::now() + Duration::from_secs(1);
            *status = "Stopping the running capture…".into();
            return true;
        }
        false
    }

    /// Returns true → caller should refresh the library.
    fn do_stop_capture(&mut self, status: &mut String) -> bool {
        self.capture.stop();
        self.preview_opened = false;
        self.capture_last_reopen_at = None;
        self.capture_stop_at = None;
        self.capture_stop_input.clear();
        self.state = CaptureState::Idle;
        *status = "Capture stopped".into();
        true
    }

    fn do_start_capture(&mut self, cfg: &Config, status: &mut String) {
        self.capture.cancel_stop_timer();
        match self
            .capture
            .start(cfg, crate::capture::FFMPEG_HARD_SAFETY_CAP)
        {
            Ok(()) => {
                self.state = CaptureState::Capturing;
                self.preview_opened = false;
                self.capture_last_reopen_at = Some(Instant::now());
                self.capture_stop_input = self.max_duration.clone();
                // The "Cap" duration is the real, user-adjustable stop target —
                // ffmpeg itself was spawned with a fixed, much longer backstop
                // (see FFMPEG_HARD_SAFETY_CAP), so this timer is what actually
                // enforces it and can be freely re-armed up or down afterward
                // via "Stop after" / Set.
                if let Some(secs) = parse_duration_secs(&self.max_duration) {
                    self.capture.arm_stop_timer(secs);
                    self.capture_stop_at = Some(Instant::now() + Duration::from_secs(secs));
                    *status = format!("Capturing… stopping in {}", fmt_secs(secs));
                } else {
                    self.capture_stop_at = None;
                    *status = "Capturing…".into();
                }
            }
            Err(e) => {
                self.state = CaptureState::Idle;
                *status = format!("Capture failed to start: {e}");
            }
        }
    }
}

fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [h, m, sec] => {
            let h: u64 = h.parse().ok()?;
            let m: u64 = m.parse().ok()?;
            let sec: u64 = sec.parse().ok()?;
            Some(h * 3600 + m * 60 + sec)
        }
        [m, sec] => {
            let m: u64 = m.parse().ok()?;
            let sec: u64 = sec.parse().ok()?;
            Some(m * 60 + sec)
        }
        [sec] => sec.parse().ok(),
        _ => None,
    }
}

fn fmt_secs(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}
