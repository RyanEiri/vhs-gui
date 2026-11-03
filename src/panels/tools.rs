//! Startup tool check and the Tools window: which external programs vhs-gui found, where,
//! and which version, plus the VapourSynth plugins and Python modules the scripts load. The
//! check spawns `--version` subprocesses, so it runs on a background thread and the UI polls
//! for the result.

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

    pub fn show_window(&mut self, ctx: &egui::Context) {
        let mut recheck = false;
        let checking = self.checking();
        egui::Window::new("Tools")
            .open(&mut self.open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let Some(report) = &self.report else {
                    ui.label("Checking…");
                    return;
                };
                egui::Grid::new("tools_grid")
                    .num_columns(4)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for s in &report.tools {
                            ui.label(s.tool.binary())
                                .on_hover_text(format!("Used for: {}", s.tool.needed_for()));
                            match &s.path {
                                Some(p) => {
                                    ui.label(egui::RichText::new("✓").color(egui::Color32::GREEN));
                                    ui.label(s.version.as_deref().unwrap_or("-"));
                                    ui.label(egui::RichText::new(p.display().to_string()).small());
                                }
                                None => {
                                    ui.label(
                                        egui::RichText::new("⚠ missing")
                                            .color(egui::Color32::YELLOW),
                                    );
                                    ui.label("-");
                                    ui.label(egui::RichText::new(s.tool.hint()).small().weak());
                                }
                            }
                            ui.end_row();
                        }
                    });

                ui.add_space(8.0);
                ui.label(egui::RichText::new("VapourSynth plugins and modules").strong());
                egui::Grid::new("vs_stack_grid")
                    .num_columns(3)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for s in &report.stack {
                            let kind = match s.need.kind {
                                Kind::Plugin => "plugin",
                                Kind::Module => "module",
                            };
                            ui.label(s.need.name)
                                .on_hover_text(format!("Needed for: {}", s.need.used_by));
                            match &s.found {
                                Some(p) => {
                                    ui.label(egui::RichText::new("✓").color(egui::Color32::GREEN));
                                    ui.label(egui::RichText::new(p.display().to_string()).small());
                                }
                                None => {
                                    ui.label(
                                        egui::RichText::new("⚠ missing")
                                            .color(egui::Color32::YELLOW),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("{kind}, see docs/INSTALL.md"))
                                            .small()
                                            .weak(),
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });

                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Override a path with VHS_GUI_FFMPEG, VHS_GUI_FFPROBE, VHS_GUI_SOX, \
                         VHS_GUI_VSPIPE, VHS_GUI_REALESRGAN_ROCM or VHS_GUI_REALESRGAN_NCNN; \
                         plugin directories with VS_PLUGIN_DIRS. Files are checked for \
                         presence, not imported.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_enabled_ui(!checking, |ui| {
                    if ui.button("Recheck").clicked() {
                        recheck = true;
                    }
                });
            });
        if recheck {
            self.recheck(ctx);
        }
    }
}
