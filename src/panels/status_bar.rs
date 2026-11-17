//! The status bar: the one place that says what is running, from every view. A recording capture
//! (REC chip), the running job (label, stage, progress, time left, Pause, Stop after seg, Cancel…),
//! the last status line and the tool-problem button. Also the shared confirm-in-place button.

use std::time::{Duration, Instant};

use crate::icons;
use crate::panels::monitor::MonitorPanel;
use crate::panels::tools::ToolsPanel;
use crate::panels::upscale::UpscalePanel;
use crate::pipeline::PipelineJob;
use crate::status::{Armed, Confirm, Remaining, TimeLeft, hms};
use crate::theme::{self, color};

const BUTTON_H: f32 = 24.0;

/// Two-click button. `first` is its normal look (the caller picks the style: a red outline for
/// Cancel…, a plain button for Stop Capture). The first click swaps it, in the same place, for the
/// safe choice (`keep_label`, which takes focus) and the confirm (`confirm_label`, solid red); the
/// second click on the confirm returns `true`. Escape, the safe button or four seconds put it back.
pub fn confirm_button(
    ui: &mut egui::Ui,
    confirm: &mut Confirm,
    what: Armed,
    first: egui::Button<'_>,
    confirm_label: &str,
    keep_label: &str,
    height: f32,
) -> bool {
    let now = Instant::now();
    if confirm.is_armed(what, now) {
        if let Some(left) = confirm.expires_in(now) {
            ui.ctx()
                .request_repaint_after(left + Duration::from_millis(50));
        }
        let keep = ui.add(egui::Button::new(keep_label).min_size(egui::vec2(0.0, height)));
        if !keep.has_focus() {
            keep.request_focus();
        }
        let ok =
            ui.add(theme::danger_solid_button(confirm_label).min_size(egui::vec2(0.0, height)));
        if keep.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            confirm.disarm();
        }
        if ok.clicked() {
            confirm.disarm();
            return true;
        }
        false
    } else {
        if ui.add(first).clicked() {
            confirm.arm(what, now);
        }
        false
    }
}

/// Overall progress of a job, `0.0..=1.0`: segments done plus the part of the current one for an
/// upscale, otherwise the time-based progress. `None` when the job cannot say.
pub fn overall_progress(job: &PipelineJob) -> Option<f32> {
    if job.is_upscale {
        let total = job.total_segments as f32;
        if total <= 0.0 {
            return None;
        }
        let part = job.segment_progress().unwrap_or(0.0);
        Some(((job.completed_segments as f32 + part) / total).min(1.0))
    } else {
        job.progress()
    }
}

#[derive(Default)]
pub struct StatusBar {
    /// The control waiting for its second click (job Cancel, Stop Capture, Stop it).
    pub confirm: Confirm,
    time_left: TimeLeft,
    /// The label of the job the estimate belongs to, so a new job starts a new estimate.
    tracked: Option<String>,
}

impl StatusBar {
    /// Feed this frame's progress to the time-left estimate. Call once per frame.
    pub fn tick(&mut self, job: Option<&PipelineJob>) {
        match job {
            Some(j) => {
                if self.tracked.as_deref() != Some(j.label.as_str()) {
                    self.time_left.reset();
                    self.tracked = Some(j.label.clone());
                }
                if let Some(p) = overall_progress(j) {
                    self.time_left.record(Instant::now(), p);
                }
            }
            None => {
                self.tracked = None;
                self.time_left.reset();
            }
        }
    }

    // egui 0.34: no non-deprecated top-level Panel::show; revisit on egui upgrade.
    #[allow(deprecated)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        monitor: &MonitorPanel,
        upscale: &mut UpscalePanel,
        tools: &mut ToolsPanel,
        status: &mut String,
    ) {
        let problems = tools.problems();
        egui::Panel::bottom("status")
            .exact_size(theme::size::STATUS_BAR_HEIGHT)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(color::BG_RAISED)
                    .stroke(egui::Stroke::new(1.0_f32, color::STROKE_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if let Some(rec) = monitor.recording_summary() {
                        rec_chip(ui, &rec);
                    }
                    if let Some(job) = upscale.pipeline_job_mut() {
                        self.job_block(ui, job, status);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !problems.is_empty() {
                            let n = problems.len();
                            let text = egui::RichText::new(format!(
                                "{} {n} tool problem{}",
                                icons::WARNING,
                                if n == 1 { "" } else { "s" }
                            ))
                            .color(color::WARNING)
                            .small();
                            let b = egui::Button::new(text)
                                .fill(color::TRANSPARENT)
                                .stroke(egui::Stroke::new(1.0_f32, color::WARNING))
                                .min_size(egui::vec2(0.0, BUTTON_H));
                            if ui.add(b).on_hover_text(problems.join("\n")).clicked() {
                                tools.open = true;
                            }
                        }
                        status_line(ui, status);
                    });
                });
            });
    }

    fn job_block(&mut self, ui: &mut egui::Ui, job: &mut PipelineJob, status: &mut String) {
        let paused = job.paused;
        if paused {
            theme::badge(ui, "PAUSED", theme::Tone::Warning, false);
        } else {
            ui.add(egui::Spinner::new().size(14.0).color(color::ACCENT));
        }

        // Label, then the stage; the part that gives way when the bar is narrow.
        let reserved = 130.0 + 90.0 + 90.0 + if job.is_upscale { 270.0 } else { 150.0 };
        let room = (ui.available_width() - reserved - 230.0).max(90.0);
        ui.scope(|ui| {
            ui.set_max_width(room);
            let mut text = job.label.clone();
            if let Some(stage) = job.stage_text() {
                text = format!("{stage} · {text}");
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .family(egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()))
                        .size(12.0)
                        .color(color::TEXT_STRONG),
                )
                .truncate(),
            );
        });

        if let Some(p) = overall_progress(job) {
            let fill = if paused {
                color::WARNING
            } else {
                color::ACCENT
            };
            ui.allocate_ui(egui::vec2(120.0, 8.0), |ui| {
                theme::progress(ui, p, 8.0, fill);
            });
        }

        let readout = if job.is_upscale && job.total_segments > 0 {
            Some(format!(
                "seg {}/{}",
                (job.completed_segments + 1).min(job.total_segments),
                job.total_segments
            ))
        } else {
            overall_progress(job).map(|p| format!("{:.0}%", p * 100.0))
        };
        match readout {
            Some(r) => ui.label(
                egui::RichText::new(r)
                    .monospace()
                    .size(11.5)
                    .color(color::TEXT_STRONG),
            ),
            None => ui.label(
                egui::RichText::new(job.elapsed_str())
                    .monospace()
                    .size(11.5)
                    .color(color::TEXT_WEAK),
            ),
        };
        match self.time_left.remaining(paused) {
            Remaining::Secs(s) => {
                ui.label(
                    egui::RichText::new(format!("{} left", hms(s)))
                        .monospace()
                        .size(11.5)
                        .color(color::TEXT_WEAK),
                );
            }
            Remaining::Paused => {
                ui.label(
                    egui::RichText::new("paused")
                        .monospace()
                        .size(11.5)
                        .color(color::WARNING),
                );
            }
            Remaining::Unknown => {}
        }

        // Controls.
        let (icon, tip) = if paused {
            (icons::PLAY, "Resume")
        } else {
            (icons::PAUSE, "Pause")
        };
        let pause = egui::Button::new(format!("{icon} {tip}")).min_size(egui::vec2(0.0, BUTTON_H));
        if ui.add(pause).clicked() {
            job.toggle_pause();
        }
        if job.is_upscale {
            if job.stopping_after_segment() {
                ui.label(
                    egui::RichText::new("stopping after this segment…")
                        .italics()
                        .size(11.5)
                        .color(color::TEXT_WEAK),
                );
            } else {
                let stop = egui::Button::new("Stop after seg").min_size(egui::vec2(0.0, BUTTON_H));
                if ui.add(stop).clicked() {
                    job.request_stop_after_segment();
                    *status = "Stopping after current segment…".into();
                }
            }
        }
        ui.separator();
        let cancel = theme::danger_button(format!("{} Cancel…", icons::CANCEL))
            .min_size(egui::vec2(0.0, BUTTON_H));
        if confirm_button(
            ui,
            &mut self.confirm,
            Armed::JobCancel,
            cancel,
            "Confirm cancel",
            "Keep running",
            BUTTON_H,
        ) {
            job.cancel();
            *status = "Cancelling…".into();
        }
    }
}

/// The recording chip: dot, REC and the elapsed time, white on the recording red.
fn rec_chip(ui: &mut egui::Ui, elapsed: &str) {
    egui::Frame::new()
        .fill(color::DANGER_FILL)
        .corner_radius(egui::CornerRadius::same(theme::size::RADIUS_BADGE))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                egui::RichText::new(icons::CAPTURE)
                    .color(color::ON_DANGER)
                    .size(12.0),
            );
            ui.label(
                egui::RichText::new("REC")
                    .family(egui::FontFamily::Name(theme::FAMILY_BOLD.into()))
                    .size(11.0)
                    .color(color::ON_DANGER),
            );
            ui.label(
                egui::RichText::new(elapsed)
                    .monospace()
                    .size(11.5)
                    .color(color::ON_DANGER),
            );
        });
}

/// The last thing that happened, with a check when it was a success.
fn status_line(ui: &mut egui::Ui, status: &str) {
    if status.is_empty() {
        return;
    }
    // Right-to-left: the text first so it is the rightmost, then its check on its left.
    let ok = status.ends_with("finished");
    ui.add(
        egui::Label::new(
            egui::RichText::new(status)
                .size(11.5)
                .color(color::TEXT_WEAK),
        )
        .truncate(),
    );
    if ok {
        ui.label(egui::RichText::new(icons::OK).color(color::SUCCESS));
    }
}
