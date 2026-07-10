use std::time::{Duration, Instant};

use crate::config::Config;
use crate::mpv_view::MpvView;
use crate::panels::ViewMode;
use crate::panels::monitor::{CaptureState, MonitorPanel};
use crate::panels::upscale::UpscalePanel;
use crate::persist::AppSettings;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("VHS_GUI_GIT_HASH");
const GIT_BRANCH: &str = env!("VHS_GUI_GIT_BRANCH");
const GIT_DIRTY: &str = env!("VHS_GUI_GIT_DIRTY");

pub struct App {
    cfg: Config,
    mpv: MpvView,
    monitor: MonitorPanel,
    upscale: UpscalePanel,
    view_mode: ViewMode,
    status: String,
    /// When Some, a settings save is due at this instant (debounced 750ms).
    save_due_at: Option<Instant>,
    about_open: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let cfg = Config::default();
        let mut mpv = MpvView::new(cc)?;
        mpv.wire_repaint(cc.egui_ctx.clone());

        let mut monitor = MonitorPanel::new(&cfg);
        let mut upscale = UpscalePanel::new(&cfg);
        let mut view_mode = ViewMode::Monitor;

        // Restore persisted settings; apply V4L2 preset to hardware on startup.
        let saved = AppSettings::load();
        saved.apply_to(&mut view_mode, &mut monitor.v4l2, &mut upscale.settings);

        Ok(Self {
            monitor,
            upscale,
            mpv,
            view_mode,
            status: String::new(),
            cfg,
            save_due_at: None,
            about_open: false,
        })
    }

    /// Arm the 750ms debounced save timer.
    fn arm_save(&mut self) {
        self.save_due_at = Some(Instant::now() + Duration::from_millis(750));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match self.view_mode {
                ViewMode::Monitor => {
                    let needs_refresh = self.monitor.toolbar_section(
                        ui,
                        &mut self.mpv,
                        &self.cfg,
                        &mut self.status,
                    );
                    if needs_refresh {
                        self.upscale.refresh_library(&self.cfg);
                    }

                    ui.separator();

                    if self.monitor.state != CaptureState::Capturing
                        && ui
                            .button(if self.mpv.state.paused { "▶" } else { "⏸" })
                            .clicked()
                    {
                        self.mpv.toggle_pause();
                    }

                    ui.separator();

                    ui.label("Cap:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.monitor.max_duration)
                            .desired_width(70.0),
                    );

                    // Compact upscale status when a job is running in the background.
                    if let Some(job) = self.upscale.pipeline_job() {
                        ui.separator();
                        let summary = if job.total_segments > 0 {
                            format!(
                                "⬆ {}/{} segs  {}",
                                job.completed_segments,
                                job.total_segments,
                                job.elapsed_str(),
                            )
                        } else {
                            format!("⬆ {}", job.elapsed_str())
                        };
                        ui.label(egui::RichText::new(summary).small().weak());
                    }
                }
                ViewMode::Upscale => {
                    self.upscale.toolbar_section(ui, &mut self.status);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(&self.status).weak().small());
            });
        });
    }

    /// Returns true if the active view changed (triggers a settings save).
    // egui 0.34: no non-deprecated top-level Panel::show; revisit on egui upgrade.
    #[allow(deprecated)]
    fn show_rail(&mut self, ctx: &egui::Context) -> bool {
        let mut view_changed = false;
        egui::Panel::left("rail")
            .exact_size(44.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    let mon_sel =
                        egui::Button::selectable(self.view_mode == ViewMode::Monitor, "⏺");
                    if ui.add(mon_sel).on_hover_text("Monitor").clicked()
                        && self.view_mode != ViewMode::Monitor
                    {
                        self.view_mode = ViewMode::Monitor;
                        view_changed = true;
                    }
                    ui.add_space(4.0);
                    let up_sel = egui::Button::selectable(self.view_mode == ViewMode::Upscale, "⬆");
                    if ui.add(up_sel).on_hover_text("Upscale").clicked()
                        && self.view_mode != ViewMode::Upscale
                    {
                        self.view_mode = ViewMode::Upscale;
                        view_changed = true;
                    }

                    // Settings toggles — per-view.
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    match self.view_mode {
                        ViewMode::Monitor => {
                            let sel = egui::Button::selectable(self.monitor.input_panel_open, "⚙");
                            if ui.add(sel).on_hover_text("Input Settings").clicked() {
                                self.monitor.input_panel_open = !self.monitor.input_panel_open;
                            }
                        }
                        ViewMode::Upscale => {
                            let sel =
                                egui::Button::selectable(self.upscale.settings_panel_open, "⚙");
                            if ui.add(sel).on_hover_text("Upscale Settings").clicked() {
                                self.upscale.settings_panel_open =
                                    !self.upscale.settings_panel_open;
                            }
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    let about_sel = egui::Button::selectable(self.about_open, "ℹ");
                    if ui.add(about_sel).on_hover_text("About").clicked() {
                        self.about_open = !self.about_open;
                    }
                });
            });
        view_changed
    }

    fn show_about_window(&mut self, ctx: &egui::Context) {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };

        egui::Window::new("About vhs-gui")
            .open(&mut self.about_open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("about_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Version:");
                        ui.label(VERSION);
                        ui.end_row();

                        ui.label("Commit:");
                        ui.label(format!("{GIT_BRANCH}@{GIT_HASH}{GIT_DIRTY}"));
                        ui.end_row();

                        ui.label("Build:");
                        ui.label(profile);
                        ui.end_row();

                        ui.label("Running from:");
                        ui.label(exe);
                        ui.end_row();
                    });
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    // egui 0.34: no non-deprecated top-level Panel::show/CentralPanel::show; revisit
    // on egui upgrade (Panel::show_inside requires a parent Ui that eframe's
    // App::update doesn't provide at the top level).
    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 1. Render mpv frame into off-screen FBO (must precede any UI draw calls).
        if let Some(gl) = frame.gl() {
            self.mpv.render_frame(gl);
        }

        // 2. Flush any pending debounced save.
        if self
            .save_due_at
            .map(|t| Instant::now() >= t)
            .unwrap_or(false)
        {
            self.save_due_at = None;
            AppSettings::capture_from(&self.view_mode, &self.monitor.v4l2, &self.upscale.settings)
                .save();
        }

        // 3. Poll capture state machine.
        if self
            .monitor
            .poll(ctx, &mut self.mpv, &self.cfg, &mut self.status)
        {
            self.upscale.refresh_library(&self.cfg);
        }

        // 4. Poll upscale/pipeline job (keeps running even when Monitor view is active).
        self.upscale.poll(ctx, &self.cfg, &mut self.status);

        // 5. Build UI.
        egui::Panel::top("toolbar").show(ctx, |ui| {
            self.toolbar(ui);
        });

        // Icon-only left rail: Monitor (⏺) | Upscale (⬆) | About (ℹ).
        if self.show_rail(ctx) {
            self.arm_save();
        }

        if self.about_open {
            self.show_about_window(ctx);
        }

        // Monitor view: collapsible Input settings panel (V4L2 hardware controls).
        if self.view_mode == ViewMode::Monitor && self.monitor.input_panel_open {
            let resp = egui::Panel::left("input")
                .resizable(true)
                .default_size(220.0)
                .show(ctx, |ui| self.monitor.show_input_panel(ui));
            if resp.inner {
                self.arm_save();
            }
        }

        // Upscale view: collapsible Settings panel (11 upscale knobs).
        if self.view_mode == ViewMode::Upscale && self.upscale.settings_panel_open {
            let resp = egui::Panel::left("upscale_settings")
                .resizable(true)
                .default_size(240.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .show(ui, |ui| self.upscale.show_settings_panel(ui))
                        .inner
                });
            if resp.inner {
                self.arm_save();
            }
        }

        // Upscale view: file library sidebar.
        if self.view_mode == ViewMode::Upscale {
            egui::Panel::left("library")
                .resizable(true)
                .default_size(220.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Library");
                        if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                            self.upscale.refresh_library(&self.cfg);
                        }
                    });
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            self.upscale.show_sidebar(
                                ui,
                                ctx,
                                &mut self.mpv,
                                &self.cfg,
                                &mut self.status,
                            );
                        });
                });
        }

        // Central panel: upscale preview only when Upscale view is active;
        // Monitor view always shows mpv so capture and upscale can run concurrently.
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.view_mode == ViewMode::Upscale && self.upscale.is_upscaling() {
                self.upscale.show_central(ui);
            } else {
                let cap_osd = if self.monitor.state == CaptureState::Capturing {
                    Some(self.monitor.capture.elapsed_str())
                } else {
                    None
                };
                self.mpv.show(ui, cap_osd.as_deref());
                if self.mpv.state.duration > 0.0 {
                    let pos = format_time(self.mpv.state.time_pos);
                    let dur = format_time(self.mpv.state.duration);
                    ui.label(format!("{pos} / {dur}"));
                } else if self.mpv.state.idle {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(
                                "No media\nSelect a file from the library or start monitoring",
                            )
                            .weak(),
                        );
                    });
                }
            }
        });
    }
}

fn format_time(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sc = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sc:02}")
    } else {
        format!("{m}:{sc:02}")
    }
}
