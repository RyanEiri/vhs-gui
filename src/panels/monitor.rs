use std::time::{Duration, Instant};

use crate::capture::CaptureController;
use crate::config::Config;
use crate::mpv_view::{MpvView, Source};
use crate::v4l2::V4l2Controls;

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
            capture: CaptureController::new(cfg.capture_pgid_file(), cfg.archival_dir()),
            state: CaptureState::Idle,
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
            CaptureState::Idle => {
                if ui.button("Monitor").clicked() {
                    let dev = cfg.v4l2_device.clone();
                    mpv.open(&Source::V4l2(dev));
                    self.state = CaptureState::Monitoring;
                    *status = "Monitoring live signal".into();
                }
                if ui.button("Start Capture").clicked() {
                    self.begin_capture(mpv, cfg, status);
                }
            }
            CaptureState::Monitoring => {
                ui.label(egui::RichText::new("● MONITOR").color(egui::Color32::GREEN));
                if ui.button("Start Capture").clicked() {
                    self.begin_capture(mpv, cfg, status);
                }
                if ui.button("Stop Monitor").clicked() {
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

        // Releasing → Capturing once mpv is idle or 1 s timeout.
        if self.state == CaptureState::Releasing {
            let timed_out = self
                .releasing_at
                .map(|t| t.elapsed() > Duration::from_millis(1000))
                .unwrap_or(true);
            if mpv.state.idle || timed_out {
                self.releasing_at = None;
                self.do_start_capture(cfg, status);
            } else {
                ctx.request_repaint();
            }
        }

        if self.state != CaptureState::Capturing {
            return false;
        }

        ctx.request_repaint_after(Duration::from_secs(1));

        // Open the UDP preview once ffmpeg confirms it's running.
        if !self.preview_opened && self.capture.is_running() {
            mpv.open(&Source::Udp("udp://127.0.0.1:23000?pkt_size=1316".into()));
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
                mpv.open(&Source::Udp("udp://127.0.0.1:23000?pkt_size=1316".into()));
                self.capture_last_reopen_at = Some(Instant::now());
            }
        }

        // GUI-side timed stop (backup for the OS timer thread).
        if self
            .capture_stop_at
            .map(|d| Instant::now() >= d)
            .unwrap_or(false)
        {
            self.capture.stop();
            self.end_capture_to_monitoring(mpv, cfg);
            *status = "Capture stopped (timer) — monitoring live signal".into();
            return true;
        }

        // Natural end: ffmpeg exited on its own (hit -t cap, or normal finish).
        if !self.capture.is_running() {
            self.end_capture_to_monitoring(mpv, cfg);
            *status = "Capture ended — monitoring live signal".into();
            return true;
        }

        false
    }

    /// Shared cleanup after a timer-driven capture end (natural `-t` safety
    /// cap, or the GUI "Stop after" deadline) — the archival file is done,
    /// but the user likely still wants the live feed rather than a frozen
    /// last frame, so hand the V4L2 device straight back to Monitoring
    /// instead of dropping to Idle. Manual "Stop Capture" is unaffected —
    /// it still goes to Idle via `do_stop_capture`.
    fn end_capture_to_monitoring(&mut self, mpv: &mut MpvView, cfg: &Config) {
        self.preview_opened = false;
        self.capture_last_reopen_at = None;
        self.capture_stop_at = None;
        self.capture_stop_input.clear();
        mpv.open(&Source::V4l2(cfg.v4l2_device.clone()));
        self.state = CaptureState::Monitoring;
    }

    fn begin_capture(&mut self, mpv: &mut MpvView, cfg: &Config, status: &mut String) {
        if self.state == CaptureState::Monitoring {
            mpv.stop();
            self.state = CaptureState::Releasing;
            self.releasing_at = Some(Instant::now());
            *status = "Releasing device…".into();
        } else {
            self.do_start_capture(cfg, status);
        }
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
            .start(&cfg.capture_script(), &self.max_duration)
        {
            Ok(()) => {
                self.state = CaptureState::Capturing;
                self.preview_opened = false;
                self.capture_last_reopen_at = Some(Instant::now());
                self.capture_stop_at = None;
                self.capture_stop_input = self.max_duration.clone();
                *status = "Capturing…".into();
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
