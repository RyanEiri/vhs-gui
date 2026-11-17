use std::time::{Duration, Instant};

use crate::capture::{self, CaptureController, OrphanCapture};
use crate::config::Config;
use crate::icons;
use crate::monitor_feed::MonitorFeed;
use crate::mpv_view::{MpvView, Source};
use crate::panels::status_bar::confirm_button;
use crate::status::{Armed, Confirm, capture_progress, hms};
use crate::theme;
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
        }
    }

    /// Draw the V4L2 hardware-control sliders inside a caller-supplied panel.
    /// Returns true if any control value changed this frame (triggers a settings save).
    /// The V4L2 sliders below the Session block; collapsed to start with while a capture is
    /// running, to leave the Session block room.
    pub fn show_input_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let recording = self.state == CaptureState::Capturing || !self.orphans.is_empty();
        self.v4l2.show_panel(ui, recording)
    }

    /// The elapsed time of a recording, for the status bar's REC chip: this app's own capture, or
    /// one found running from an earlier run.
    pub fn recording_summary(&self) -> Option<String> {
        match self.state {
            CaptureState::Capturing => Some(self.capture.elapsed_str()),
            CaptureState::Idle => self.orphans.first().map(|o| fmt_secs(o.age_secs)),
            _ => None,
        }
    }

    /// The stop time of the running capture in seconds from its start: elapsed plus what is left.
    pub fn stop_total_secs(&self) -> Option<u64> {
        let elapsed = self.capture.elapsed_secs()?;
        let left = self
            .capture_stop_at?
            .saturating_duration_since(Instant::now())
            .as_secs();
        Some(elapsed + left)
    }

    /// A capture from an earlier run is still recording.
    pub fn has_orphan(&self) -> bool {
        self.state == CaptureState::Idle && !self.orphans.is_empty()
    }

    /// The Session block at the top of the Capture side panel: what the device is doing and the
    /// controls for it. Returns true when the library should be refreshed (a capture was stopped).
    pub fn show_session(
        &mut self,
        ui: &mut egui::Ui,
        mpv: &mut MpvView,
        cfg: &Config,
        status: &mut String,
        confirm: &mut Confirm,
    ) -> bool {
        match self.state {
            CaptureState::Idle if !self.orphans.is_empty() => {
                self.orphan_block(ui, cfg, status, confirm)
            }
            CaptureState::Idle | CaptureState::Monitoring => {
                self.start_block(ui, mpv, cfg, status);
                false
            }
            CaptureState::Releasing => {
                session_header(ui, None);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0).color(theme::color::ACCENT));
                    ui.label(egui::RichText::new("Releasing device…").italics());
                });
                false
            }
            CaptureState::Capturing => self.recording_block(ui, status, confirm),
        }
    }

    /// Idle and Monitoring: the state badge, Max length, and the buttons that start things.
    fn start_block(
        &mut self,
        ui: &mut egui::Ui,
        mpv: &mut MpvView,
        cfg: &Config,
        status: &mut String,
    ) {
        let monitoring = self.state == CaptureState::Monitoring;
        session_header(ui, None);
        ui.horizontal(|ui| {
            if monitoring {
                theme::badge(ui, "LIVE", theme::Tone::Success, true);
                ui.label(
                    egui::RichText::new("Monitoring, not recording")
                        .size(11.5)
                        .color(theme::color::TEXT_WEAK),
                );
            } else {
                theme::badge(ui, "IDLE", theme::Tone::Neutral, false);
                ui.label(
                    egui::RichText::new("Device free")
                        .size(11.5)
                        .color(theme::color::TEXT_WEAK),
                );
            }
        });
        ui.horizontal(|ui| {
            ui.label("Max length");
            ui.add(
                egui::TextEdit::singleline(&mut self.max_duration)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(92.0)
                    .hint_text("HH:MM:SS"),
            )
            .on_hover_text("Applies to the next capture only; Stop after changes the running one");
        });
        if !monitoring {
            let b = egui::Button::new(format!("{} Monitor", icons::VIEWER));
            if full_width(ui, b, theme::size::PRIMARY_HEIGHT - 2.0).clicked() {
                match self.start_monitor(mpv, cfg) {
                    Ok(()) => *status = "Monitoring live signal".into(),
                    Err(e) => *status = format!("Monitor failed to start: {e}"),
                }
            }
        }
        let start = theme::danger_solid_button(format!("{} Start Capture", icons::CAPTURE));
        if full_width(ui, start, theme::size::PRIMARY_HEIGHT).clicked() {
            self.begin_capture(cfg, status);
        }
        if monitoring {
            let stop = egui::Button::new(format!("{} Stop Monitor", icons::STOP));
            if full_width(ui, stop, theme::size::PRIMARY_HEIGHT - 2.0).clicked() {
                self.feed.stop();
                mpv.stop();
                self.state = CaptureState::Idle;
                *status = "Idle".into();
            }
        }
    }

    /// Recording: the clock, how far it is to the stop time, the file, Stop after and Stop Capture.
    fn recording_block(
        &mut self,
        ui: &mut egui::Ui,
        status: &mut String,
        confirm: &mut Confirm,
    ) -> bool {
        let mut needs_refresh = false;
        let elapsed = self.capture.elapsed_secs().unwrap_or(0);
        let remaining = self
            .capture_stop_at
            .map(|d| d.saturating_duration_since(Instant::now()).as_secs());

        session_header(ui, Some(("REC", theme::Tone::DangerSolid, true)));
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(hms(elapsed))
                    .text_style(egui::TextStyle::Name(theme::STYLE_CLOCK.into()))
                    .color(theme::color::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("elapsed")
                    .size(11.5)
                    .color(theme::color::TEXT_WEAK),
            );
        });
        if let Some((total, fraction)) = capture_progress(elapsed, remaining) {
            theme::progress(ui, fraction, 8.0, theme::color::DANGER);
            ui.horizontal(|ui| {
                let mono = |t: String, c| egui::RichText::new(t).monospace().size(11.5).color(c);
                ui.label(mono(
                    format!("stops {}", hms(total)),
                    theme::color::TEXT_WEAK,
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let left = total.saturating_sub(elapsed);
                    ui.label(mono(
                        format!("{} left", hms(left)),
                        theme::color::TEXT_STRONG,
                    ));
                });
            });
        }
        if let Some(name) = self.capture.file_name() {
            ui.label(
                egui::RichText::new("Recording to")
                    .size(11.5)
                    .color(theme::color::TEXT_WEAK),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .monospace()
                        .size(10.5)
                        .color(theme::color::TEXT_NORMAL),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }

        ui.horizontal(|ui| {
            ui.label("Stop after");
            let input = ui.add(
                egui::TextEdit::singleline(&mut self.capture_stop_input)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(92.0)
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
        });

        let stop = egui::Button::new(format!("{} Stop Capture", icons::STOP)).min_size(egui::vec2(
            ui.available_width(),
            theme::size::PRIMARY_HEIGHT,
        ));
        if confirm_button(
            ui,
            confirm,
            Armed::StopCapture,
            stop,
            "Confirm stop",
            "Keep recording",
            theme::size::PRIMARY_HEIGHT,
        ) {
            needs_refresh = self.do_stop_capture(status);
        }
        needs_refresh
    }

    /// A capture that outlived an earlier run: say so, and offer Reattach or Stop it. Returns true
    /// when the library should refresh.
    fn orphan_block(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &Config,
        status: &mut String,
        confirm: &mut Confirm,
    ) -> bool {
        let Some(orphan) = self.orphans.first().cloned() else {
            return false;
        };
        session_header(ui, Some(("STILL RECORDING", theme::Tone::Warning, false)));
        ui.label(
            egui::RichText::new(
                "A capture started by an earlier run of vhs-gui is still recording.",
            )
            .size(11.5)
            .color(theme::color::TEXT_NORMAL),
        );
        ui.label(
            egui::RichText::new(format!("Running for {}", fmt_secs(orphan.age_secs)))
                .size(11.5)
                .color(theme::color::TEXT_WEAK),
        );
        ui.label(
            egui::RichText::new("File")
                .size(11.5)
                .color(theme::color::TEXT_WEAK),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(orphan.name())
                    .monospace()
                    .size(10.5)
                    .color(theme::color::TEXT_NORMAL),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );

        if orphan.is_group_leader() {
            let reattach = theme::primary_button(format!("{} Reattach", icons::REATTACH));
            if full_width(ui, reattach, theme::size::PRIMARY_HEIGHT).clicked() {
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
            caption(ui, "Show its progress, stop it from here, set a stop time.");
        }
        let stop_it = theme::danger_button(format!("{} Stop it", icons::CANCEL)).min_size(
            egui::vec2(ui.available_width(), theme::size::PRIMARY_HEIGHT),
        );
        let stopped = confirm_button(
            ui,
            confirm,
            Armed::StopOrphan,
            stop_it,
            "Confirm stop",
            "Keep recording",
            theme::size::PRIMARY_HEIGHT,
        );
        caption(ui, "Interrupt ffmpeg so the file is finalized.");
        if stopped {
            for o in &self.orphans {
                o.stop();
            }
            self.orphan_scan_due = Instant::now() + Duration::from_secs(1);
            *status = "Stopping the running capture…".into();
            return true;
        }
        false
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

/// The "SESSION" label with an optional badge on the right.
fn session_header(ui: &mut egui::Ui, badge: Option<(&str, theme::Tone, bool)>) {
    ui.horizontal(|ui| {
        ui.label(theme::section_label("Session"));
        if let Some((text, tone, dot)) = badge {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::badge(ui, text, tone, dot);
            });
        }
    });
}

/// A button as wide as the panel.
fn full_width(ui: &mut egui::Ui, button: egui::Button<'_>, height: f32) -> egui::Response {
    ui.add(button.min_size(egui::vec2(ui.available_width(), height)))
}

/// A line of help text under a button.
fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(10.5)
            .color(theme::color::TEXT_WEAK),
    );
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
