//! Startup tool check and the Tools window: which external programs vhs-gui found, where,
//! and which version, plus the VapourSynth plugins and Python modules the scripts load. The
//! check spawns `--version` subprocesses, so it runs on a background thread and the UI polls
//! for the result.

use crate::icons;
use crate::theme;
use crossbeam_channel::{Receiver, TryRecvError};

use crate::tools::{self, ToolStatus};
use crate::vs_stack::{self, Kind, StackStatus};

struct Report {
    tools: Vec<ToolStatus>,
    stack: Vec<StackStatus>,
}

pub struct ToolsPanel {
    pub open: bool,
    report: Option<Report>,
    rx: Option<Receiver<Report>>,
}

impl ToolsPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut panel = Self {
            open: false,
            report: None,
            rx: None,
        };
        panel.recheck(ctx);
        panel
    }

    /// Start a fresh check; a check already in flight is left to finish.
    pub fn recheck(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Report {
                tools: tools::check_all(),
                stack: vs_stack::check(),
            });
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Pick up a finished check. Call once per frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(report) => {
                self.report = Some(report);
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            // The worker died without a result; allow another attempt.
            Err(TryRecvError::Disconnected) => self.rx = None,
        }
    }

    pub fn checking(&self) -> bool {
        self.rx.is_some()
    }

    /// Problems with the toolset. Empty while the first check is still running, so the
    /// toolbar does not flash a warning at startup.
    pub fn problems(&self) -> Vec<String> {
        let Some(r) = &self.report else {
            return Vec::new();
        };
        let mut out = tools::problems(&r.tools);
        out.extend(vs_stack::problems(&r.stack));
        out
    }

    /// Draw the window. Returns true when the user asked to set up VapourSynth. `setup_running`
    /// disables both buttons while that job runs (the status bar carries it).
    // egui 0.34: Window has no replacement for show(ctx); revisit on egui upgrade.
    pub fn show_window(&mut self, ctx: &egui::Context, setup_running: bool) -> bool {
        let mut recheck = false;
        let mut setup = false;
        let checking = self.checking();
        // The window never grows past the app window: its rows scroll instead.
        let screen = ctx.content_rect().height();
        let body_max = (screen - 34.0 - 30.0 - 96.0 - 52.0).max(120.0);
        let frame = egui::Frame::new()
            .fill(theme::color::BG_RAISED)
            .stroke(egui::Stroke::new(1.0_f32, theme::color::STROKE_WIDGET))
            .corner_radius(egui::CornerRadius::same(theme::size::RADIUS_WINDOW))
            .shadow(ctx.global_style().visuals.window_shadow)
            .inner_margin(egui::Margin::same(12));
        egui::Window::new("Tools")
            .open(&mut self.open)
            .resizable(false)
            .collapsible(false)
            .frame(frame)
            .default_width(640.0)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 12.0))
            .show(ctx, |ui| {
                ui.set_width(616.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                let Some(report) = &self.report else {
                    summary_strip(ui, Summary::Checking);
                    return;
                };
                let missing = report.tools.iter().filter(|t| t.path.is_none()).count()
                    + report.stack.iter().filter(|s| s.found.is_none()).count();
                let total = report.tools.len() + report.stack.len();
                let vs_missing = report.stack.iter().any(|s| s.found.is_none())
                    || report
                        .tools
                        .iter()
                        .any(|t| t.path.is_none() && t.tool == crate::tools::Tool::Vspipe);
                if checking {
                    summary_strip(ui, Summary::Checking);
                } else if missing == 0 {
                    summary_strip(ui, Summary::AllFound(total));
                } else {
                    summary_strip(ui, Summary::Missing(missing));
                }

                egui::ScrollArea::vertical()
                    .max_height(body_max)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(theme::section_label("Programs"));
                        for s in &report.tools {
                            let r = tool_row(
                                ui,
                                s.tool.binary(),
                                s.path.as_deref(),
                                s.version.as_deref().unwrap_or("-"),
                                s.tool.hint(),
                            );
                            r.on_hover_text(format!("Used for: {}", s.tool.needed_for()));
                        }
                        ui.add_space(4.0);
                        ui.label(theme::section_label("VapourSynth plugins and modules"));
                        for s in &report.stack {
                            let kind = match s.need.kind {
                                Kind::Plugin => "plugin",
                                Kind::Module => "module",
                            };
                            let hint = format!("{kind}, see docs/INSTALL.md");
                            let r = tool_row(ui, s.need.name, s.found.as_deref(), "", &hint);
                            r.on_hover_text(format!("Needed for: {}", s.need.used_by));
                        }
                    });

                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "Override a path with VHS_GUI_FFMPEG, VHS_GUI_FFPROBE, VHS_GUI_SOX, \
                         VHS_GUI_VSPIPE, VHS_GUI_REALESRGAN_ROCM or VHS_GUI_REALESRGAN_NCNN; \
                         plugin directories with VS_PLUGIN_DIRS. Files are checked for \
                         presence, not imported.",
                    )
                    .size(10.5)
                    .color(theme::color::TEXT_WEAK),
                );
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!checking && !setup_running, |ui| {
                        let b = egui::Button::new(format!("{} Recheck", icons::REFRESH));
                        if ui.add(b).clicked() {
                            recheck = true;
                        }
                        // The one next step when VapourSynth is incomplete, otherwise plain.
                        let label = format!("{} Set up VapourSynth…", icons::TOOLS);
                        let tip = "Creates a private Python environment in \
                                   ~/.local/share/vhs-gui/vapoursynth with VapourSynth R81, the \
                                   mvtools, znedi3 and vivtc plugins, havsfunc and vsutil (about \
                                   25 MB from PyPI, pinned by hash). Needs python3-venv and a \
                                   network connection. Once it exists vhs-gui uses it instead of \
                                   the system VapourSynth; delete the folder to go back.";
                        let button = if vs_missing {
                            theme::primary_button(label)
                        } else {
                            egui::Button::new(label)
                        };
                        if ui.add(button).on_hover_text(tip).clicked() {
                            setup = true;
                        }
                    });
                    if setup_running {
                        ui.add(egui::Spinner::new().size(14.0).color(theme::color::ACCENT));
                        ui.label(
                            egui::RichText::new("Setting up… progress is in the status bar")
                                .size(11.0)
                                .color(theme::color::TEXT_WEAK),
                        );
                    }
                });
            });
        if recheck {
            self.recheck(ctx);
        }
        setup
    }
}

enum Summary {
    Checking,
    AllFound(usize),
    Missing(usize),
}

/// The 30 px strip at the top: all found (green), N missing (amber), or a spinner while checking.
fn summary_strip(ui: &mut egui::Ui, summary: Summary) {
    let (fill, fg, icon, text) = match summary {
        Summary::Checking => (
            theme::color::WIDGET_IDLE,
            theme::color::TEXT_NORMAL,
            icons::WORKING,
            "Checking…".to_owned(),
        ),
        Summary::AllFound(n) => (
            theme::color::SUCCESS_DIM,
            theme::color::SUCCESS,
            icons::OK,
            format!("All {n} items found"),
        ),
        Summary::Missing(n) => (
            theme::color::WARNING_DIM,
            theme::color::WARNING,
            icons::WARNING,
            format!("{n} item{} missing", if n == 1 { "" } else { "s" }),
        ),
    };
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(theme::size::RADIUS_WIDGET),
        fill,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{icon}  {text}"),
        egui::FontId::new(12.0, egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into())),
        fg,
    );
}

/// One 24 px row: name, status, version and path. A missing item tints the whole row and shows the
/// hint where the path would be.
fn tool_row(
    ui: &mut egui::Ui,
    name: &str,
    path: Option<&std::path::Path>,
    version: &str,
    hint: &str,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let mid = rect.center().y;
    let (name_x, status_x, version_x, path_x) = (
        rect.left() + 8.0,
        rect.left() + 8.0 + 170.0,
        rect.left() + 8.0 + 170.0 + 80.0,
        rect.left() + 8.0 + 170.0 + 80.0 + 64.0,
    );
    if path.is_none() {
        p.rect_filled(
            rect,
            egui::CornerRadius::same(theme::size::RADIUS_WIDGET),
            theme::color::WARNING_DIM,
        );
    }
    p.text(
        egui::pos2(name_x, mid),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::monospace(12.0),
        theme::color::TEXT_STRONG,
    );
    match path {
        Some(path) => {
            p.text(
                egui::pos2(status_x, mid),
                egui::Align2::LEFT_CENTER,
                format!("{} found", icons::OK),
                egui::FontId::new(11.5, egui::FontFamily::Proportional),
                theme::color::SUCCESS,
            );
            p.text(
                egui::pos2(version_x, mid),
                egui::Align2::LEFT_CENTER,
                version,
                egui::FontId::monospace(11.5),
                theme::color::TEXT_NORMAL,
            );
            p.text(
                egui::pos2(path_x, mid),
                egui::Align2::LEFT_CENTER,
                path.display().to_string(),
                egui::FontId::monospace(10.5),
                theme::color::TEXT_WEAK,
            );
        }
        None => {
            p.text(
                egui::pos2(status_x, mid),
                egui::Align2::LEFT_CENTER,
                format!("{} missing", icons::WARNING),
                egui::FontId::new(11.5, egui::FontFamily::Name(theme::FAMILY_BOLD.into())),
                theme::color::WARNING,
            );
            p.text(
                egui::pos2(version_x, mid),
                egui::Align2::LEFT_CENTER,
                hint,
                egui::FontId::new(10.5, egui::FontFamily::Proportional),
                theme::color::TEXT_NORMAL,
            );
        }
    }
    resp
}
