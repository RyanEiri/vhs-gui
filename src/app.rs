use crate::icons;
use crate::theme;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::mpv_view::MpvView;
use crate::panels::ViewMode;
use crate::panels::monitor::{CaptureState, MonitorPanel};
use crate::panels::notice::{self, Notices};
use crate::panels::status_bar::StatusBar;
use crate::panels::tools::ToolsPanel;
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
    tools: ToolsPanel,
    view_mode: ViewMode,
    status: String,
    /// When Some, a settings save is due at this instant (debounced 750ms).
    save_due_at: Option<Instant>,
    about_open: bool,
    paths_open: bool,
    /// Raw text-field contents for the Working Directories window. Blank
    /// means "no override" — see `AppSettings::capture_from`.
    paths_work_root_input: String,
    status_bar: StatusBar,
    notices: Notices,
    about_banner: Option<egui::TextureHandle>,
    about_icon: Option<egui::TextureHandle>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        // Loaded before Config so persisted path overrides (if any) are baked
        // into cfg from the start, rather than applied after the fact.
        let saved = AppSettings::load();
        let cfg = Config::from_paths(&saved.paths);

        theme::install_fonts(&cc.egui_ctx, theme::bundled_fonts());
        theme::apply(&cc.egui_ctx);

        let mut mpv = MpvView::new(cc)?;
        mpv.wire_repaint(cc.egui_ctx.clone());

        let mut monitor = MonitorPanel::new(&cfg);
        let mut upscale = UpscalePanel::new(&cfg);
        let tools = ToolsPanel::new(&cc.egui_ctx);
        let mut view_mode = ViewMode::Monitor;

        // Restore remaining persisted settings; apply V4L2 preset to hardware on startup.
        saved.apply_to(&mut view_mode, &mut monitor.v4l2, &mut upscale.settings);
        view_mode = view_mode.at_startup(upscale.is_upscaling());
        upscale
            .library
            .set_section_overrides(saved.library_sections.clone());

        let paths_work_root_input = saved.paths.work_root.clone().unwrap_or_default();

        Ok(Self {
            monitor,
            upscale,
            tools,
            mpv,
            view_mode,
            status: String::new(),
            cfg,
            save_due_at: None,
            about_open: false,
            paths_open: false,
            paths_work_root_input,
            status_bar: StatusBar::default(),
            notices: Notices::default(),
            about_banner: None,
            about_icon: None,
        })
    }

    /// Arm the 750ms debounced save timer.
    fn arm_save(&mut self) {
        self.save_due_at = Some(Instant::now() + Duration::from_millis(750));
    }

    /// The view title, 13 px bold.
    fn title(ui: &mut egui::Ui, text: &str) {
        ui.label(
            egui::RichText::new(text)
                .family(egui::FontFamily::Name(theme::FAMILY_BOLD.into()))
                .size(13.0)
                .color(theme::color::TEXT_STRONG),
        );
    }

    /// The 40 px toolbar, different in each view. What is running is in the status bar, not here.
    // egui 0.34: no non-deprecated top-level Panel::show; revisit on egui upgrade.
    #[allow(deprecated)]
    fn show_toolbar(&mut self, ctx: &egui::Context) {
        // While recording, the Capture toolbar turns red so it cannot be missed.
        let recording =
            self.view_mode == ViewMode::Monitor && self.monitor.state == CaptureState::Capturing;
        let fill = if recording {
            theme::color::DANGER_FILL
        } else {
            theme::color::BG_RAISED
        };
        egui::Panel::top("toolbar")
            .exact_size(theme::size::TOOLBAR_HEIGHT)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(fill)
                    .stroke(egui::Stroke::new(1.0_f32, theme::color::STROKE_SUBTLE))
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 10,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    match self.view_mode {
                        ViewMode::Monitor => self.capture_toolbar(ui),
                        ViewMode::Library => self.library_toolbar(ui),
                        ViewMode::Upscale => self.upscale_toolbar(ui),
                    }
                });
            });
    }

    fn capture_toolbar(&mut self, ui: &mut egui::Ui) {
        if self.monitor.state == CaptureState::Capturing {
            self.recording_toolbar(ui);
            return;
        }
        Self::title(ui, "Capture");
        let (text, tone, dot) = if self.monitor.has_orphan() {
            ("ORPHAN CAPTURE", theme::Tone::Warning, false)
        } else {
            match self.monitor.state {
                CaptureState::Monitoring => ("LIVE", theme::Tone::Success, true),
                CaptureState::Releasing => ("RELEASING", theme::Tone::Warning, false),
                _ => ("IDLE", theme::Tone::Neutral, false),
            }
        };
        theme::badge(ui, text, tone, dot);
        if self.monitor.state == CaptureState::Monitoring {
            ui.label(
                egui::RichText::new("Input changes apply to the hardware as you drag")
                    .size(11.5)
                    .color(theme::color::TEXT_WEAK),
            );
        }
    }

    /// The red recording bar: RECORDING and the file on the left, the clock and stop time on the right.
    fn recording_toolbar(&mut self, ui: &mut egui::Ui) {
        let on = theme::color::ON_DANGER;
        ui.label(egui::RichText::new(icons::CAPTURE).size(14.0).color(on));
        ui.label(
            egui::RichText::new("RECORDING")
                .family(egui::FontFamily::Name(theme::FAMILY_BOLD.into()))
                .size(13.0)
                .color(on),
        );
        let stop = self.monitor.stop_total_secs();
        let elapsed = self.monitor.capture.elapsed_str();
        let name = self.monitor.capture.file_name();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(total) = stop {
                ui.label(
                    egui::RichText::new(format!("/ {}", crate::status::hms(total)))
                        .monospace()
                        .size(11.5)
                        .color(on),
                );
            }
            ui.label(
                egui::RichText::new(elapsed)
                    .family(egui::FontFamily::Name(theme::FAMILY_MONO_BOLD.into()))
                    .size(18.0)
                    .color(on),
            );
            if let Some(name) = name {
                ui.add(
                    egui::Label::new(egui::RichText::new(name).monospace().size(11.5).color(on))
                        .truncate(),
                );
            }
        });
    }

    fn library_toolbar(&mut self, ui: &mut egui::Ui) {
        let (icon, tip) = if self.mpv.state.paused {
            (icons::PLAY, "Play")
        } else {
            (icons::PAUSE, "Pause")
        };
        let b = egui::Button::new(icon).min_size(egui::vec2(26.0, 26.0));
        if ui.add(b).on_hover_text(tip).clicked() {
            self.mpv.toggle_pause();
        }
        let Some(entry) = self.upscale.library.selected_entry() else {
            ui.label(
                egui::RichText::new("Select a file")
                    .size(12.0)
                    .color(theme::color::TEXT_WEAK),
            );
            return;
        };
        theme::badge(
            ui,
            &entry.kind.label().to_uppercase(),
            theme::Tone::Accent,
            false,
        );
        let name = entry.name.clone();
        let busy = self.upscale.is_busy();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let trash = theme::danger_button(format!("{} Move to Trash…", icons::TRASH));
            if ui
                .add_enabled(!busy, trash)
                .on_disabled_hover_text("Not while a job is running")
                .clicked()
            {
                self.upscale.request_trash();
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&name)
                        .monospace()
                        .size(11.5)
                        .color(theme::color::TEXT_STRONG),
                )
                .truncate(),
            );
        });
    }

    fn upscale_toolbar(&mut self, ui: &mut egui::Ui) {
        Self::title(ui, "Upscale");
        let running = self.upscale.is_upscaling();
        if running {
            theme::badge(ui, "RUNNING", theme::Tone::Accent, true);
            if let Some(job) = self.upscale.pipeline_job() {
                ui.label(
                    egui::RichText::new(&job.label)
                        .family(egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()))
                        .size(12.0)
                        .color(theme::color::TEXT_STRONG),
                );
            }
        } else {
            theme::badge(ui, "NO JOB", theme::Tone::Neutral, false);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let open = self.upscale.settings_panel_open;
            let b = egui::Button::selectable(open, format!("{} Settings", icons::SETTINGS));
            if ui.add(b).on_hover_text("Upscale Settings").clicked() {
                self.upscale.settings_panel_open = !open;
            }
        });
    }

    /// One rail item: an icon over a label, filled while its view or window is current. `dot` is an
    /// 8 px marker at the top right (a recording, a tool problem).
    fn rail_item(
        ui: &mut egui::Ui,
        icon: &str,
        label: &str,
        active: bool,
        dot: Option<egui::Color32>,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(48.0, 46.0), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let fill = if active {
                theme::color::WIDGET_ACTIVE
            } else if resp.hovered() {
                theme::color::WIDGET_HOVERED
            } else {
                theme::color::TRANSPARENT
            };
            p.rect_filled(rect, egui::CornerRadius::same(6), fill);
            let fg = if active {
                theme::color::ACCENT
            } else {
                theme::color::TEXT_WEAK
            };
            p.text(
                egui::pos2(rect.center().x, rect.top() + 16.0),
                egui::Align2::CENTER_CENTER,
                icon,
                egui::FontId::new(18.0, egui::FontFamily::Proportional),
                fg,
            );
            p.text(
                egui::pos2(rect.center().x, rect.bottom() - 9.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::new(10.0, egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into())),
                if active {
                    theme::color::TEXT_STRONG
                } else {
                    theme::color::TEXT_WEAK
                },
            );
            if let Some(c) = dot {
                p.circle_filled(egui::pos2(rect.right() - 7.0, rect.top() + 7.0), 4.0, c);
            }
        }
        resp
    }

    /// Returns true if the active view changed (triggers a settings save).
    // egui 0.34: no non-deprecated top-level Panel::show; revisit on egui upgrade.
    #[allow(deprecated)]
    fn show_rail(&mut self, ctx: &egui::Context) -> bool {
        let mut view_changed = false;
        let recording = self.monitor.recording_summary().is_some();
        let tool_problem = !self.tools.problems().is_empty();
        egui::Panel::left("rail")
            .exact_size(theme::size::RAIL_WIDTH)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::color::BG_RAISED)
                    .stroke(egui::Stroke::new(1.0_f32, theme::color::STROKE_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(4, 6)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let mut view_item =
                    |ui: &mut egui::Ui, view: ViewMode, icon: &str, label: &str, dot| {
                        let current = self.view_mode == view;
                        if Self::rail_item(ui, icon, label, current, dot).clicked() && !current {
                            self.view_mode = view;
                            view_changed = true;
                        }
                    };
                let rec_dot = recording.then_some(theme::color::DANGER);
                view_item(ui, ViewMode::Monitor, icons::CAPTURE, "Capture", rec_dot);
                view_item(ui, ViewMode::Library, icons::LIBRARY, "Library", None);
                view_item(ui, ViewMode::Upscale, icons::UPSCALE, "Upscale", None);

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if Self::rail_item(ui, icons::ABOUT, "About", self.about_open, None).clicked() {
                        self.about_open = !self.about_open;
                    }
                    if Self::rail_item(ui, icons::FOLDERS, "Folders", self.paths_open, None)
                        .on_hover_text("Working Directories")
                        .clicked()
                    {
                        self.paths_open = !self.paths_open;
                    }
                    let tools_dot = tool_problem.then_some(theme::color::WARNING);
                    if Self::rail_item(ui, icons::TOOLS, "Tools", self.tools.open, tools_dot)
                        .clicked()
                    {
                        self.tools.open = !self.tools.open;
                    }
                });
            });
        view_changed
    }

    /// The failure banner and the success toast, in two panels stacked above the status bar.
    // egui 0.34: no non-deprecated top-level Panel::show; revisit on egui upgrade.
    #[allow(deprecated)]
    fn show_notices(&mut self, ctx: &egui::Context) {
        if let Some(toast) = &self.notices.toast {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            let can_show = toast.output.is_some();
            let mut show = false;
            let mut dismiss = false;
            egui::Panel::bottom("toast")
                .exact_size(28.0)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(theme::color::SUCCESS_DIM)
                        .inner_margin(egui::Margin::symmetric(12, 0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        show = notice::show_toast(ui, toast, can_show);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            dismiss = ui.small_button(icons::CANCEL).clicked();
                        });
                    });
                });
            let output = toast.output.clone();
            if show {
                if let Some(p) = output {
                    self.upscale.select_path(&p);
                    self.view_mode = ViewMode::Library;
                    self.arm_save();
                }
                self.notices.toast = None;
            } else if dismiss {
                self.notices.toast = None;
            }
        }
        if let Some(failure) = &self.notices.failure {
            let (mut open, mut dismiss) = (false, false);
            egui::Panel::bottom("failure")
                .exact_size(50.0)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(theme::color::DANGER_DIM)
                        .stroke(egui::Stroke::new(1.0_f32, theme::color::DANGER))
                        .inner_margin(egui::Margin::symmetric(12, 0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        (open, dismiss) = notice::show_banner(ui, failure);
                    });
                });
            if open {
                // The log folder or file, in the desktop's own viewer.
                let _ = std::process::Command::new("xdg-open")
                    .arg(&failure.log)
                    .spawn();
            }
            if dismiss {
                self.notices.failure = None;
            }
        }
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

        let banner = self.about_banner(ctx);
        let frame = window_frame(ctx);
        egui::Window::new("About vhs-gui")
            .open(&mut self.about_open)
            .resizable(false)
            .collapsible(false)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(400.0);
                if let Some(tex) = &banner {
                    let w = ui.available_width();
                    let h = w * tex.size_vec2().y / tex.size_vec2().x;
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::from_handle(tex))
                            .fit_to_exact_size(egui::vec2(w, h))
                            .corner_radius(egui::CornerRadius::same(theme::size::RADIUS_WIDGET)),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if let Some(icon) = &self.about_icon {
                        ui.add(
                            egui::Image::new(egui::load::SizedTexture::from_handle(icon))
                                .fit_to_exact_size(egui::vec2(44.0, 44.0)),
                        );
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.heading("vhs-gui");
                        ui.label(
                            egui::RichText::new("GPL-3.0")
                                .size(11.5)
                                .color(theme::color::TEXT_WEAK),
                        );
                    });
                });
                ui.add_space(8.0);
                let commit = format!("{GIT_BRANCH}@{GIT_HASH}{GIT_DIRTY}");
                for (label, value) in [
                    ("Version", VERSION),
                    ("Commit", commit.as_str()),
                    ("Build", profile),
                    ("Running from", exe.as_str()),
                ] {
                    ui.horizontal_top(|ui| {
                        ui.add_sized(
                            egui::vec2(96.0, 18.0),
                            egui::Label::new(
                                egui::RichText::new(label)
                                    .size(11.5)
                                    .color(theme::color::TEXT_WEAK),
                            ),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(value)
                                    .monospace()
                                    .size(11.5)
                                    .color(theme::color::TEXT_NORMAL),
                            )
                            .wrap(),
                        );
                    });
                }
            });
    }

    /// The banner and the icon as textures, decoded the first time About opens.
    fn about_banner(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if self.about_icon.is_none() {
            let icon = png_to_color_image(include_bytes!("../assets/icon.png"))?;
            self.about_icon =
                Some(ctx.load_texture("about_icon", icon, egui::TextureOptions::LINEAR));
        }
        if self.about_banner.is_none() {
            let img = png_to_color_image(include_bytes!("../assets/banner.png"))?;
            self.about_banner =
                Some(ctx.load_texture("about_banner", img, egui::TextureOptions::LINEAR));
        }
        self.about_banner.clone()
    }

    /// Videos directory (data root) is deliberately not editable here:
    /// the vhs-cli scripts hardcode $HOME/Videos with no env override, so
    /// changing it on the GUI side alone would desync the library view from
    /// where the scripts put captures. The work root is safe — every consumer
    /// reads cfg fresh per action, so a change here takes effect on the next
    /// button click, no restart needed.
    fn show_paths_window(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        let frame = window_frame(ctx);
        egui::Window::new("Working Directories")
            .open(&mut self.paths_open)
            .resizable(false)
            .collapsible(false)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(
                    egui::RichText::new("Leave a field blank to use its env var / default.")
                        .size(11.5)
                        .color(theme::color::TEXT_WEAK),
                );
                ui.add_space(4.0);
                ui.label("Upscale work root:");
                changed |= path_field(
                    ui,
                    &mut self.paths_work_root_input,
                    &Config::default_work_root(),
                );
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Reset to default").clicked() {
                            self.paths_work_root_input.clear();
                            changed = true;
                        }
                    });
                });
            });

        if changed {
            self.cfg.work_root = if self.paths_work_root_input.trim().is_empty() {
                Config::default_work_root()
            } else {
                PathBuf::from(self.paths_work_root_input.trim())
            };
            self.arm_save();
        }
    }
}

/// A path text field with a "not found" line when the folder is missing and a hint showing the
/// resolved default when left blank. Returns true if the field changed.
fn path_field(ui: &mut egui::Ui, buf: &mut String, default: &std::path::Path) -> bool {
    let resp = ui.add(
        egui::TextEdit::singleline(buf)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .hint_text(default.to_string_lossy()),
    );
    let effective = if buf.trim().is_empty() {
        default.to_path_buf()
    } else {
        PathBuf::from(buf.trim())
    };
    if !effective.is_dir() {
        ui.label(
            egui::RichText::new(format!("{} not found", icons::WARNING))
                .family(egui::FontFamily::Name(theme::FAMILY_BOLD.into()))
                .size(11.5)
                .color(theme::color::WARNING),
        );
    }
    resp.changed()
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
            AppSettings::capture_from(
                &self.view_mode,
                &self.monitor.v4l2,
                &self.upscale.settings,
                &self.paths_work_root_input,
                self.upscale.library.section_overrides(),
            )
            .save();
        }

        // 3. Poll capture state machine.
        if self
            .monitor
            .poll(ctx, &mut self.mpv, &self.cfg, &mut self.status)
        {
            self.upscale.refresh_library(&self.cfg);
        }

        // Pick up the finished startup tool check.
        self.tools.poll();

        // 4. Poll upscale/pipeline job (keeps running even when Monitor view is active).
        self.upscale.poll(ctx, &self.cfg, &mut self.status);
        if let Some(done) = self.upscale.take_finished() {
            self.notices
                .push(done, &self.cfg.log_dir(), std::time::Instant::now());
        }
        self.notices.expire(std::time::Instant::now());

        // 5. Build UI. The status bar goes first so it spans the full width, then the rail, the
        // toolbar, the side panels and the central area.
        self.status_bar.tick(self.upscale.pipeline_job());
        self.status_bar.show(
            ctx,
            &self.monitor,
            &mut self.upscale,
            &mut self.tools,
            &mut self.status,
        );

        self.show_notices(ctx);

        if self.show_rail(ctx) {
            self.arm_save();
        }
        self.show_toolbar(ctx);

        if self.about_open {
            self.show_about_window(ctx);
        }

        if self.paths_open {
            self.show_paths_window(ctx);
        }

        // The setup job's progress is in the status bar, in whichever view the user is.
        let setup_running = self
            .upscale
            .pipeline_job()
            .is_some_and(|j| j.label == crate::vs_setup::LABEL);
        if self.tools.open && self.tools.show_window(ctx, setup_running) {
            self.upscale.launch_vs_setup(&self.cfg, &mut self.status);
        }
        if self.upscale.take_vs_setup_finished() {
            self.tools.recheck(ctx);
        }

        match self.view_mode {
            // Capture: the Session block above the V4L2 input controls, beside the live picture.
            ViewMode::Monitor => {
                let mut refresh = false;
                let resp = egui::Panel::left("input")
                    .resizable(true)
                    .default_size(theme::size::SIDE_PANEL_WIDTH)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                refresh = self.monitor.show_session(
                                    ui,
                                    &mut self.mpv,
                                    &self.cfg,
                                    &mut self.status,
                                    &mut self.status_bar.confirm,
                                );
                                ui.separator();
                                self.monitor.show_input_panel(ui)
                            })
                            .inner
                    });
                if refresh {
                    self.upscale.refresh_library(&self.cfg);
                }
                if resp.inner {
                    self.arm_save();
                }
            }
            // Library: the file list with the actions for the selected file beneath it. Three
            // fixed regions (header, list, actions), so opening an inline panel never moves the list.
            ViewMode::Library => {
                let mut sections_changed = false;
                egui::Panel::left("library")
                    .resizable(true)
                    .default_size(theme::size::SIDE_PANEL_WIDTH)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Library")
                                    .family(egui::FontFamily::Name(theme::FAMILY_BOLD.into()))
                                    .size(13.0)
                                    .color(theme::color::TEXT_STRONG),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} files",
                                    self.upscale.library.entries.len()
                                ))
                                .monospace()
                                .size(11.0)
                                .color(theme::color::TEXT_WEAK),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(icons::REFRESH)
                                                .fill(theme::color::TRANSPARENT)
                                                .min_size(egui::vec2(24.0, 24.0)),
                                        )
                                        .on_hover_text("Refresh")
                                        .clicked()
                                    {
                                        self.upscale.refresh_library(&self.cfg);
                                    }
                                },
                            );
                        });
                        ui.add_space(2.0);
                        if self.upscale.has_selection() {
                            egui::Panel::bottom("library_actions")
                                .resizable(false)
                                .frame(
                                    egui::Frame::new()
                                        .fill(theme::color::BG_PANEL)
                                        .stroke(egui::Stroke::new(
                                            1.0_f32,
                                            theme::color::STROKE_WIDGET,
                                        ))
                                        .inner_margin(egui::Margin::same(10)),
                                )
                                .show_inside(ui, |ui| {
                                    self.upscale
                                        .show_actions(ui, ctx, &self.cfg, &mut self.status);
                                });
                        }
                        egui::CentralPanel::default()
                            .frame(egui::Frame::new())
                            .show_inside(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        sections_changed = self.upscale.show_list(
                                            ui,
                                            &mut self.mpv,
                                            &mut self.status,
                                        );
                                    });
                            });
                    });
                if sections_changed {
                    self.arm_save();
                }
            }
            // Upscale: the settings panel, open by default; the toolbar's Settings toggle closes it.
            ViewMode::Upscale => {
                if self.upscale.settings_panel_open {
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
            }
        }

        // Starting an upscale from the Library shows it in the Upscale view.
        if self.upscale.take_upscale_started() && self.view_mode != ViewMode::Upscale {
            self.view_mode = ViewMode::Upscale;
            self.arm_save();
        }

        // Central area: the Upscale view shows the running job (or says there is none); Capture and
        // Library show the player, so a capture and an upscale can run at the same time.
        let mut to_library = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.view_mode == ViewMode::Upscale {
                if self.upscale.is_upscaling() {
                    let paused = self.upscale.pipeline_job().is_some_and(|j| j.paused);
                    let remaining = self.status_bar.remaining(paused);
                    self.upscale
                        .show_central(ui, remaining, &mut self.status_bar.confirm);
                } else {
                    to_library = show_no_upscale(ui);
                }
            } else {
                let cap_osd = if self.monitor.state == CaptureState::Capturing {
                    Some(self.monitor.capture.elapsed_str())
                } else {
                    None
                };
                self.mpv.show(ui, cap_osd.as_deref());
                if self.mpv.state.idle && self.mpv.state.duration <= 0.0 {
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
        if to_library {
            self.view_mode = ViewMode::Library;
            self.arm_save();
        }
    }

    /// Called once on shutdown (window close, Cmd/Ctrl+Q, etc. — but also any
    /// abnormal event-loop exit, e.g. the GL/Wayland surface becoming invalid
    /// during a `fix-siddhartha-plasmashell` restart on display wake). We used
    /// to `job.cancel()` an in-progress upscale here so nothing kept running
    /// invisibly after a deliberate quit — but that made the GUI's own
    /// stability a single point of failure for a multi-hour GPU job, and
    /// `on_exit` can't tell a deliberate quit apart from an environmental one.
    /// Upscale jobs are already `.process_group(0)`-isolated and
    /// checkpoint-resumable, and now write a pgid lock file
    /// (`with_upscale_tracking`) so a later vhs-gui process can find and
    /// reattach to a job that outlived its GUI (see `check_lock`/
    /// `attach_running` in pipeline.rs) — so leaving it running is strictly
    /// safer than guessing wrong and killing hours of work. If the user wants
    /// it stopped, Cancel/Stop-after-Segment in the UI still do that explicitly.
    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        if self.monitor.capture.is_running() {
            self.monitor.capture.stop();
        }
        // The monitor feed holds the grabber and records nothing: it must not outlive the GUI.
        self.monitor.feed.kill();
    }
}

/// The Upscale view with nothing running. Returns true when "Go to Library" is pressed.
fn show_no_upscale(ui: &mut egui::Ui) -> bool {
    let mut go = false;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.3).max(24.0));
        ui.set_max_width(380.0);
        ui.label(
            egui::RichText::new(icons::UPSCALE)
                .size(28.0)
                .color(theme::color::TEXT_WEAK),
        );
        ui.heading("No upscale running");
        ui.label(
            egui::RichText::new(
                "Pick an Edit Master or Viewer file in Library and press one of its Upscale \
                 actions. A job from an earlier session reattaches when you press the same \
                 action again.",
            )
            .color(theme::color::TEXT_WEAK),
        );
        ui.add_space(6.0);
        go = ui
            .button(format!("{} Go to Library", icons::LIBRARY))
            .clicked();
    });
    go
}

/// The look of the floating windows: raised panel, widget stroke, 6 px corners.
fn window_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::new()
        .fill(theme::color::BG_RAISED)
        .stroke(egui::Stroke::new(1.0_f32, theme::color::STROKE_WIDGET))
        .corner_radius(egui::CornerRadius::same(theme::size::RADIUS_WINDOW))
        .shadow(ctx.global_style().visuals.window_shadow)
        .inner_margin(egui::Margin::same(12))
}

/// Decode an embedded PNG (RGB or RGBA) into an egui image; `None` if it is anything else.
fn png_to_color_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = png::Decoder::new(bytes).read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..info.buffer_size()];
    let size = [info.width as usize, info.height as usize];
    match info.color_type {
        png::ColorType::Rgba => Some(egui::ColorImage::from_rgba_unmultiplied(size, data)),
        png::ColorType::Rgb => Some(egui::ColorImage::from_rgb(size, data)),
        _ => None,
    }
}
