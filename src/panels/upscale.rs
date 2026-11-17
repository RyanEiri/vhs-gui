use std::path::PathBuf;

use crate::config::Config;
use crate::icons;
use crate::library::Library;
use crate::mpv_view::{MpvView, Source};
use crate::panels::status_bar::{confirm_button, overall_progress};
use crate::pipeline::PipelineJob;
use crate::settings::{
    Backend, BrightnessPreset, CrushPreset, UpscaleSettings, preset_models_dirs,
};
use crate::status::{Armed, Confirm, Remaining, hms};
use crate::theme;
use crate::upscale::plan::Variant;
use crate::vs_ops::VsOp;

enum CentralAction {
    Pause,
    StopAfter,
    Cancel,
}

struct UpscalePreviewTextures {
    orig: egui::TextureHandle,
    upscaled: egui::TextureHandle,
    segment: u64,
    total_segments: u64,
    frame: u64,
    segment_frames: u64,
}

/// The inline panel open in the Library's action region. Only one at a time; it replaces the action
/// groups and belongs to one file (selecting another file closes it).
enum Inline {
    None,
    Trash(PathBuf),
    Rename(PathBuf, String),
    AudioCleanup(PathBuf, crate::audio_cleanup::Params),
    Letterbox(PathBuf),
}

impl Inline {
    fn path(&self) -> Option<&PathBuf> {
        match self {
            Inline::None => None,
            Inline::Trash(p)
            | Inline::Rename(p, _)
            | Inline::AudioCleanup(p, _)
            | Inline::Letterbox(p) => Some(p),
        }
    }
}

pub struct UpscalePanel {
    pub library: Library,
    pub settings: UpscaleSettings,
    pub settings_panel_open: bool,
    pipeline: Option<PipelineJob>,
    /// A VapourSynth op to start on the running job's output when it finishes successfully
    /// (Denoise+QTGMC is a Denoise job with QTGMC chained after it). Taken on every completion,
    /// so a failed or cancelled job never starts the next stage.
    chain_after: Option<VsOp>,
    /// The inline panel open in the Library's action region, if any.
    inline: Inline,
    /// File a "Find Quiet Spot" scan is running for, and the scan itself.
    quiet_spot_scan: Option<(PathBuf, crate::audio_cleanup::QuietSpotScan)>,
    /// Letterbox detection results, applied by the Upscale and Viewer Encode buttons.
    letterbox: super::letterbox::Letterbox,
    /// An upscale was started or reattached to since the app last looked.
    upscale_started: bool,
    /// A "Set up VapourSynth" job just ended (successfully or not); the app rechecks the tools.
    vs_setup_finished: bool,
    last_preview_at: Option<std::time::Instant>,
    last_preview_frames: u64,
    preview_textures: Option<UpscalePreviewTextures>,
}

impl UpscalePanel {
    pub fn new(cfg: &Config) -> Self {
        let mut library = Library::new();
        library.refresh(cfg);
        Self {
            library,
            settings: UpscaleSettings::default(),
            settings_panel_open: false,
            pipeline: None,
            chain_after: None,
            inline: Inline::None,
            quiet_spot_scan: None,
            letterbox: Default::default(),
            vs_setup_finished: false,
            upscale_started: false,
            last_preview_at: None,
            last_preview_frames: 0,
            preview_textures: None,
        }
    }

    /// Draw the 11-knob upscale settings panel.
    /// Returns true if any knob value changed this frame.
    pub fn show_settings_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.heading("Upscale Settings");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(icons::BACK)
                    .on_hover_text("Close panel")
                    .clicked()
                {
                    self.settings_panel_open = false;
                }
            });
        });
        ui.separator();

        egui::Grid::new("upscale_settings_grid")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                // --- Backend ---
                ui.label("Backend");
                ui.horizontal(|ui| {
                    let prev = self.settings.backend.clone();
                    // Two joined segments: the active one is filled with the accent.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (b, label) in [(Backend::Rocm, "ROCm"), (Backend::Vulkan, "Vulkan")] {
                        let on = self.settings.backend == b;
                        let text = egui::RichText::new(label).color(if on {
                            theme::color::ACCENT_INK
                        } else {
                            theme::color::TEXT_NORMAL
                        });
                        let btn = egui::Button::new(text)
                            .fill(if on {
                                theme::color::ACCENT
                            } else {
                                theme::color::WIDGET_IDLE
                            })
                            .min_size(egui::vec2(72.0, 24.0));
                        if ui.add(btn).clicked() {
                            self.settings.backend = b;
                        }
                    }
                    if self.settings.backend != prev {
                        changed = true;
                        // Clamp model index to the new effective list length.
                        let len = self.settings.effective_model_list().len();
                        if self.settings.model_idx >= len {
                            self.settings.model_idx = 0;
                        }
                    }
                });
                ui.end_row();

                // --- Models dir (Vulkan only) ---
                if self.settings.backend == Backend::Vulkan {
                    ui.label("Models Dir");
                    ui.horizontal(|ui| {
                        let presets = preset_models_dirs();
                        let prev_idx = self.settings.models_dir_idx;
                        egui::ComboBox::from_id_salt("models_dir_combo")
                            .selected_text(if self.settings.models_dir_idx < presets.len() {
                                presets[self.settings.models_dir_idx].0.as_str()
                            } else {
                                "Custom"
                            })
                            .show_ui(ui, |ui| {
                                for (i, (label, _)) in presets.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.settings.models_dir_idx,
                                        i,
                                        label,
                                    );
                                }
                                ui.selectable_value(
                                    &mut self.settings.models_dir_idx,
                                    presets.len(),
                                    "Custom…",
                                );
                            });
                        if ui
                            .small_button(icons::REFRESH)
                            .on_hover_text("Rescan models")
                            .clicked()
                            || self.settings.models_dir_idx != prev_idx
                        {
                            changed = true;
                            self.settings.rescan();
                        }
                    });
                    ui.end_row();

                    // Custom dir text edit
                    if self.settings.models_dir_idx >= preset_models_dirs().len() {
                        ui.label("");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.settings.models_dir_custom)
                                .desired_width(f32::INFINITY)
                                .hint_text("/path/to/models"),
                        );
                        if r.changed() {
                            changed = true;
                        }
                        ui.end_row();
                    }
                }

                // --- Model ---
                ui.label("Model");
                ui.horizontal(|ui| {
                    // Collect into owned strings first to release the shared borrow
                    // before the mutable borrow of model_idx inside the ComboBox closure.
                    let list: Vec<String> = self
                        .settings
                        .effective_model_list()
                        .into_iter()
                        .map(|s| s.to_owned())
                        .collect();
                    let selected_label = list
                        .get(self.settings.model_idx)
                        .map(|s| s.as_str())
                        .unwrap_or("(none)");
                    let prev_idx = self.settings.model_idx;
                    egui::ComboBox::from_id_salt("model_combo")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for (i, name) in list.iter().enumerate() {
                                ui.selectable_value(&mut self.settings.model_idx, i, name.as_str());
                            }
                        });
                    // Auto-infer internal scale when model changes.
                    if self.settings.model_idx != prev_idx {
                        changed = true;
                        self.settings.sync_scale_to_model();
                    }
                    if self.settings.backend == Backend::Rocm {
                        ui.label(
                            egui::RichText::new("ROCm: PyTorch (.pth) checkpoints only")
                                .small()
                                .weak(),
                        );
                    }
                });
                ui.end_row();

                // --- Internal scale ---
                ui.label("Int. Scale");
                let fixed_scale = self
                    .settings
                    .selected_model()
                    .and_then(crate::settings::infer_scale);
                if let Some(s) = fixed_scale {
                    // Model has a known native scale — lock it and keep it in sync.
                    let prev = self.settings.internal_scale;
                    self.settings.sync_scale_to_model();
                    if self.settings.internal_scale != prev {
                        changed = true;
                    }
                    ui.add_enabled(
                        false,
                        egui::Label::new(egui::RichText::new(format!("{s}×  (fixed)")).weak()),
                    );
                } else {
                    let prev_scale = self.settings.internal_scale;
                    egui::ComboBox::from_id_salt("int_scale_combo")
                        .selected_text(self.settings.internal_scale.to_string())
                        .show_ui(ui, |ui| {
                            for s in [1u8, 2, 3, 4] {
                                ui.selectable_value(
                                    &mut self.settings.internal_scale,
                                    s,
                                    s.to_string(),
                                );
                            }
                        });
                    if self.settings.internal_scale != prev_scale {
                        changed = true;
                    }
                }
                ui.end_row();

                // --- Final scale ---
                ui.label("Final Scale");
                let prev_fscale = self.settings.final_scale;
                egui::ComboBox::from_id_salt("final_scale_combo")
                    .selected_text(format!("{}×", self.settings.final_scale))
                    .show_ui(ui, |ui| {
                        for s in [1u8, 2, 4] {
                            ui.selectable_value(&mut self.settings.final_scale, s, format!("{s}×"));
                        }
                    });
                // A 1x model (or any internal scale) limits which final scales
                // the scripts accept; correct it here so the combo shows the
                // value that will actually be used.
                self.settings.clamp_final_scale();
                if self.settings.final_scale != prev_fscale {
                    changed = true;
                }
                ui.end_row();

                // --- Luma crush ---
                ui.label("Luma Crush");
                let prev_crush = self.settings.crush.clone();
                egui::ComboBox::from_id_salt("crush_combo")
                    .selected_text(self.settings.crush.label())
                    .show_ui(ui, |ui| {
                        for preset in CrushPreset::ALL {
                            ui.selectable_value(
                                &mut self.settings.crush,
                                preset.clone(),
                                preset.label(),
                            );
                        }
                    });
                if self.settings.crush != prev_crush {
                    changed = true;
                }
                ui.end_row();

                // --- Brightness ---
                ui.label("Brightness");
                let prev_bright = self.settings.brightness.clone();
                ui.vertical(|ui| {
                    egui::ComboBox::from_id_salt("brightness_combo")
                        .selected_text(self.settings.brightness.label())
                        .show_ui(ui, |ui| {
                            for preset in BrightnessPreset::ALL {
                                ui.selectable_value(
                                    &mut self.settings.brightness,
                                    preset.clone(),
                                    preset.label(),
                                );
                            }
                        });
                    if self.settings.brightness == BrightnessPreset::Custom {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.settings.brightness_custom)
                                .desired_width(80.0)
                                .hint_text("0.03"),
                        );
                        if r.changed() {
                            changed = true;
                        }
                    }
                });
                if self.settings.brightness != prev_bright {
                    changed = true;
                }
                ui.end_row();

                // --- CRF ---
                ui.label("CRF");
                if ui
                    .add(
                        egui::Slider::new(&mut self.settings.crf, 14..=28)
                            .clamping(egui::SliderClamping::Always),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.end_row();

                // --- Segment length ---
                ui.label("Segment (s)");
                if ui
                    .add(
                        egui::Slider::new(&mut self.settings.segment_secs, 10..=120)
                            .clamping(egui::SliderClamping::Always)
                            .suffix("s"),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.end_row();

                // --- Batch size (ROCm only) ---
                if self.settings.backend == Backend::Rocm {
                    ui.label("Batch Size");
                    if ui
                        .add(
                            egui::Slider::new(&mut self.settings.batch_size, 1..=8)
                                .clamping(egui::SliderClamping::Always),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    ui.end_row();
                }
            });
        changed
    }

    pub fn refresh_library(&mut self, cfg: &Config) {
        self.library.refresh(cfg);
    }

    /// Start the private VapourSynth setup (Tools window button), unless a job is already running.
    pub fn launch_vs_setup(&mut self, cfg: &Config, status: &mut String) {
        if self.pipeline.is_some() {
            *status = "Another job is running; set up VapourSynth when it finishes".into();
            return;
        }
        match crate::vs_setup::launch(cfg, crate::vs_setup::LABEL.into()) {
            Ok(job) => {
                *status = format!(
                    "Started: {} (log in {})",
                    job.label,
                    cfg.log_dir().display()
                );
                self.pipeline = Some(job);
            }
            Err(e) => *status = format!("Could not start the VapourSynth setup: {e}"),
        }
    }

    /// True once after a VapourSynth setup job has ended.
    pub fn take_vs_setup_finished(&mut self) -> bool {
        std::mem::take(&mut self.vs_setup_finished)
    }

    pub fn is_upscaling(&self) -> bool {
        self.pipeline
            .as_ref()
            .map(|j| j.is_upscale)
            .unwrap_or(false)
    }

    /// Read-only access to the active job, for status display in other views.
    pub fn pipeline_job(&self) -> Option<&crate::pipeline::PipelineJob> {
        self.pipeline.as_ref()
    }

    /// The running job, for the status bar's Pause, Stop after seg and Cancel.
    pub fn pipeline_job_mut(&mut self) -> Option<&mut crate::pipeline::PipelineJob> {
        self.pipeline.as_mut()
    }

    /// True once after an upscale started (or was reattached to): the app then shows the Upscale view.
    pub fn take_upscale_started(&mut self) -> bool {
        std::mem::take(&mut self.upscale_started)
    }

    /// Poll the running job: update progress, upload preview textures, handle completion.
    pub fn poll(&mut self, ctx: &egui::Context, cfg: &Config, status: &mut String) {
        let job = match self.pipeline.as_mut() {
            Some(j) => j,
            None => return,
        };
        job.poll();

        if job.is_upscale && !job.done {
            const PREVIEW_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);

            if job.upscaled_frames < self.last_preview_frames {
                self.last_preview_at = None;
                self.last_preview_frames = 0;
            }

            let due = self
                .last_preview_at
                .map(|t| t.elapsed() >= PREVIEW_INTERVAL)
                .unwrap_or(true);

            if due
                && job.upscaled_frames > 0
                && let (Some(up_d), Some(fr_d)) =
                    (job.frames_up_dir.as_deref(), job.frames_dir.as_deref())
                && let Some(up_path) = latest_jpg_in_dir(up_d)
                && let Some(fname) = up_path.file_name()
            {
                let orig_path = fr_d.join(fname);
                if let (Some(orig_img), Some(up_img)) = (
                    load_jpeg_as_egui_image(&orig_path),
                    load_jpeg_as_egui_image(&up_path),
                ) {
                    let seg = job.completed_segments + 1;
                    let total_segs = job.total_segments;
                    let frame = job.upscaled_frames;
                    let seg_frames = job.segment_frames;

                    match self.preview_textures {
                        Some(ref mut t) => {
                            t.orig.set(orig_img, egui::TextureOptions::LINEAR);
                            t.upscaled.set(up_img, egui::TextureOptions::LINEAR);
                            t.segment = seg;
                            t.total_segments = total_segs;
                            t.frame = frame;
                            t.segment_frames = seg_frames;
                        }
                        None => {
                            self.preview_textures = Some(UpscalePreviewTextures {
                                orig: ctx.load_texture(
                                    "upscale_orig",
                                    orig_img,
                                    egui::TextureOptions::LINEAR,
                                ),
                                upscaled: ctx.load_texture(
                                    "upscale_up",
                                    up_img,
                                    egui::TextureOptions::LINEAR,
                                ),
                                segment: seg,
                                total_segments: total_segs,
                                frame,
                                segment_frames: seg_frames,
                            });
                        }
                    }
                    self.last_preview_at = Some(std::time::Instant::now());
                    self.last_preview_frames = job.upscaled_frames;
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }

        if job.done {
            // Captured before the job (and its output_path) is dropped below,
            // so the newly-produced file can stay selected after refresh()
            // resets the index — otherwise the action panel (and its Rename
            // button) vanishes and the user has to re-click the row.
            let reselect_path = job.output_path.clone();
            if job.label == crate::vs_setup::LABEL {
                self.vs_setup_finished = true;
            }
            let failed = job_failed(job.exit_ok, &job.output_path);
            let chained = self.chain_after.take();
            let finish_status = if failed {
                format!("{} FAILED — see log", job.label)
            } else if job.is_upscale {
                cleanup_upscale_work_dir(&job.output_path, &job.segments_dir)
                    .map(|msg| format!("{} finished — {msg}", job.label))
                    .unwrap_or_else(|| format!("{} finished", job.label))
            } else {
                format!("{} finished", job.label)
            };
            *status = finish_status;
            self.last_preview_at = None;
            self.last_preview_frames = 0;
            self.preview_textures = None;
            self.pipeline = None;
            self.library.refresh(cfg);
            if let Some(p) = &reselect_path {
                self.library.selected = self.library.entries.iter().position(|e| &e.path == p);
            }
            if let Some((op, next_input)) = chained_next(failed, chained, reselect_path) {
                self.launch_vs_op(op, next_input, cfg, status);
            }
        }
    }

    /// Draw the file list. Opens a clicked file in the player; returns true when a section was
    /// folded or unfolded (the caller saves the layout).
    pub fn show_list(&mut self, ui: &mut egui::Ui, mpv: &mut MpvView, status: &mut String) -> bool {
        let event = self.library.show(ui);
        if let Some(entry) = event.open {
            *status = format!("Opening: {}", entry.name);
            mpv.open(&Source::File(entry.path));
        }
        event.sections_changed
    }

    /// Whether a file is selected (the action region is hidden otherwise).
    pub fn has_selection(&self) -> bool {
        self.library.selected_entry().is_some()
    }

    /// A job is running, so the file actions are locked.
    pub fn is_busy(&self) -> bool {
        self.pipeline.is_some()
    }

    /// The Library toolbar's Move to Trash…: open the confirmation for the selected file.
    pub fn request_trash(&mut self) {
        if self.pipeline.is_none()
            && let Some(e) = self.library.selected_entry()
        {
            self.inline = Inline::Trash(e.path.clone());
        }
    }

    /// The Upscale view while a job runs: segment readout, time left, two progress bars and the
    /// controls on top, the before and after frames below. Above about 1500 px of width everything
    /// scales up so it reads from across the room.
    pub fn show_central(&mut self, ui: &mut egui::Ui, remaining: Remaining, confirm: &mut Confirm) {
        let k: f32 = if ui.available_width() >= 1500.0 {
            1.8
        } else {
            1.0
        };
        egui::Frame::new()
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0 * k;
                self.upscale_progress_block(ui, k, remaining, confirm);
                self.upscale_previews(ui);
            });
    }

    fn upscale_progress_block(
        &mut self,
        ui: &mut egui::Ui,
        k: f32,
        remaining: Remaining,
        confirm: &mut Confirm,
    ) {
        let Some(job) = self.pipeline.as_mut() else {
            return;
        };
        let mono = |text: String, size: f32, color: egui::Color32| {
            egui::RichText::new(text)
                .monospace()
                .size(size * k)
                .color(color)
        };
        let overall = overall_progress(job).unwrap_or(0.0);
        let total = job.total_segments;
        let current = (job.completed_segments + 1).min(total.max(1));
        let button_h = 28.0 * k;
        let narrow = ui.available_width() < 700.0;
        let right_w = if narrow { 0.0 } else { 270.0 * k };
        let gap = if narrow { 0.0 } else { 16.0 };
        let left_w = (ui.available_width() - right_w - gap).max(120.0);
        let mut action = None;
        let body = |ui: &mut egui::Ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.vertical(|ui| {
                ui.set_width(left_w);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Segment {current} of {total}"))
                            .family(egui::FontFamily::Name(theme::FAMILY_MONO_BOLD.into()))
                            .size(24.0 * k)
                            .color(theme::color::TEXT_STRONG),
                    );
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", overall * 100.0))
                            .family(egui::FontFamily::Name(theme::FAMILY_MONO_BOLD.into()))
                            .size(24.0 * k)
                            .color(theme::color::ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match remaining {
                            Remaining::Secs(s) => ui.label(mono(
                                format!("{} left", hms(s)),
                                18.0,
                                theme::color::TEXT_STRONG,
                            )),
                            Remaining::Paused => {
                                ui.label(mono("paused".into(), 18.0, theme::color::WARNING))
                            }
                            Remaining::Unknown => ui.label(""),
                        };
                    });
                });
                let fill = if job.paused {
                    theme::color::WARNING
                } else {
                    theme::color::ACCENT
                };
                theme::progress(ui, overall, 12.0 * k, fill);
                let seg = job.segment_progress().unwrap_or(0.0);
                theme::progress(ui, seg, 5.0 * k, theme::color::TEXT_WEAK);
                ui.horizontal(|ui| {
                    ui.label(mono(
                        format!(
                            "Segment {current} · frame {} of {}",
                            job.upscaled_frames, job.segment_frames
                        ),
                        11.5,
                        theme::color::TEXT_NORMAL,
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(mono(
                            format!("elapsed {}", job.elapsed_str()),
                            11.5,
                            theme::color::TEXT_WEAK,
                        ));
                    });
                });
            });
            let align = if narrow {
                egui::Align::Min
            } else {
                egui::Align::Max
            };
            ui.with_layout(egui::Layout::top_down(align), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0 * k, 8.0 * k);
                ui.horizontal(|ui| {
                    let pause = if job.paused {
                        format!("{} Resume", icons::PLAY)
                    } else {
                        format!("{} Pause", icons::PAUSE)
                    };
                    if ui
                        .add(egui::Button::new(pause).min_size(egui::vec2(0.0, button_h)))
                        .clicked()
                    {
                        action = Some(CentralAction::Pause);
                    }
                    if job.stopping_after_segment() {
                        ui.label(
                            egui::RichText::new("Stopping…")
                                .italics()
                                .size(12.0 * k)
                                .color(theme::color::TEXT_WEAK),
                        );
                    } else if ui
                        .add(
                            egui::Button::new("Stop after Segment")
                                .min_size(egui::vec2(0.0, button_h)),
                        )
                        .clicked()
                    {
                        action = Some(CentralAction::StopAfter);
                    }
                });
                let cancel = theme::danger_button(format!("{} Cancel…", icons::CANCEL))
                    .min_size(egui::vec2(0.0, button_h));
                if confirm_button(
                    ui,
                    confirm,
                    Armed::JobCancel,
                    cancel,
                    "Confirm cancel",
                    "Keep running",
                    button_h,
                ) {
                    action = Some(CentralAction::Cancel);
                }
            });
        };
        if narrow {
            ui.vertical(body);
        } else {
            ui.horizontal_top(body);
        }
        match action {
            Some(CentralAction::Pause) => job.toggle_pause(),
            Some(CentralAction::StopAfter) => job.request_stop_after_segment(),
            Some(CentralAction::Cancel) => job.cancel(),
            None => {}
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }

    /// The two frames side by side at their own aspect, with a caption above each. Spread to the
    /// bottom of the area, so extra height lands between them and the progress block.
    fn upscale_previews(&self, ui: &mut egui::Ui) {
        let aspect = self
            .preview_textures
            .as_ref()
            .map(|t| {
                let s = t.orig.size_vec2();
                s.x / s.y.max(1.0)
            })
            .unwrap_or(4.0 / 3.0);
        let gap = 16.0;
        let cap_h = 20.0;
        let avail = ui.available_size();
        let max_w = ((avail.x - gap) / 2.0).max(40.0);
        let max_h = (avail.y - cap_h - 4.0).max(60.0);
        let (pw, ph) = if max_w / aspect <= max_h {
            (max_w, max_w / aspect)
        } else {
            (max_h * aspect, max_h)
        };
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(
            egui::vec2(avail.x, ph + cap_h),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let caption = |t: String| {
                        egui::RichText::new(t)
                            .monospace()
                            .size(11.5)
                            .color(theme::color::TEXT_NORMAL)
                    };
                    let size_text = |h: &egui::TextureHandle| {
                        let s = h.size();
                        format!("{}×{}", s[0], s[1])
                    };
                    let (orig_t, up_t) = match &self.preview_textures {
                        Some(t) => (
                            format!("Original  {}", size_text(&t.orig)),
                            format!("Upscaled  {}", size_text(&t.upscaled)),
                        ),
                        None => ("Original".to_owned(), "Upscaled".to_owned()),
                    };
                    for (i, caption_text) in [orig_t, up_t].into_iter().enumerate() {
                        ui.vertical(|ui| {
                            ui.label(caption(caption_text));
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(pw, ph), egui::Sense::hover());
                            ui.painter()
                                .rect_filled(rect, 0.0, theme::color::VIDEO_BLACK);
                            match &self.preview_textures {
                                Some(t) => {
                                    let h = if i == 0 { &t.orig } else { &t.upscaled };
                                    egui::Image::new(egui::load::SizedTexture::from_handle(h))
                                        .paint_at(ui, rect);
                                }
                                None if i == 1 => {
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "Upscaling…\nPreview frames will appear shortly",
                                        egui::FontId::new(12.0, egui::FontFamily::Proportional),
                                        theme::color::TEXT_WEAK,
                                    );
                                }
                                None => {}
                            }
                            ui.painter().rect_stroke(
                                rect,
                                0.0,
                                egui::Stroke::new(1.0_f32, theme::color::STROKE_SUBTLE),
                                egui::StrokeKind::Inside,
                            );
                        });
                    }
                });
            },
        );
    }

    // -----------------------------------------------------------------------
    // Launch helpers
    // -----------------------------------------------------------------------

    /// Launch the native Denoise (`captures/stabilized/<stem>_<time>_STABLE.mkv`). With `then`, that
    /// VapourSynth op is started on the denoised file once this job succeeds.
    fn launch_denoise(
        &mut self,
        input: PathBuf,
        then: Option<VsOp>,
        cfg: &Config,
        status: &mut String,
    ) {
        let name = input
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("input")
            .to_string();
        let label = match then {
            Some(op) => format!("Denoise+{} {name}", op.label_verb()),
            None => format!("Denoise {name}"),
        };
        match crate::denoise::launch(&input, cfg, label) {
            Ok(crate::denoise::Launch::Started(mut job, output)) => {
                job.output_path = Some(output);
                *status = format!("Started: {}", job.label);
                self.chain_after = then;
                self.pipeline = Some(*job);
            }
            Ok(crate::denoise::Launch::Exists(output)) => {
                *status = format!(
                    "Denoised copy already exists, not overwritten: {}",
                    output.display()
                );
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    /// Launch one of the 5 native VapourSynth deinterlace/telecine ops
    /// (QTGMC, IVTC, IVTC+Decomb, Field Align, VDecimate) directly — no
    /// bash wrapper script involved.
    fn launch_vs_op(&mut self, op: VsOp, input: PathBuf, cfg: &Config, status: &mut String) {
        let name = input
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("input")
            .to_string();
        let label = format!("{} {name}", op.label_verb());
        match crate::vs_ops::launch(op, &input, cfg, label) {
            Ok(mut job) => {
                job.output_path = Some(crate::vs_ops::output_path(op, &input));
                *status = format!("Started: {}", job.label);
                self.pipeline = Some(job);
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    /// Launch the native viewer encode (H.264 + AAC derivative in `captures/viewer/`). An existing
    /// viewer copy is left alone, as the script it replaces did.
    fn launch_viewer_encode(&mut self, input: PathBuf, cfg: &Config, status: &mut String) {
        let name = input
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("input")
            .to_string();
        let crop = self.letterbox.crop_for(&input);
        match crate::viewer_encode::launch(&input, cfg, format!("Viewer Encode {name}"), crop) {
            Ok(crate::viewer_encode::Launch::Started(mut job, output)) => {
                job.output_path = Some(output);
                *status = format!("Started: {}", job.label);
                if let Some(c) = crop {
                    *status = format!("{status} (cropped to {}x{})", c.w, c.h);
                }
                self.pipeline = Some(*job);
            }
            Ok(crate::viewer_encode::Launch::Exists(output)) => {
                *status = format!(
                    "Viewer copy already exists, not overwritten: {}",
                    output.display()
                );
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    /// Launch native A/V drift correction (`fix_sync`). Output is written
    /// beside the input as `<stem>_SYNC.mkv`.
    fn launch_fix_sync(&mut self, input: PathBuf, cfg: &Config, status: &mut String) {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        let dir = input.parent().unwrap_or_else(|| std::path::Path::new("."));
        let output = dir.join(format!("{stem}_SYNC.mkv"));
        let name = input
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("input")
            .to_string();
        match crate::fix_sync::launch(&input, &output, cfg, format!("Fix A/V Sync {name}")) {
            Ok(Some(mut job)) => {
                job.output_path = Some(output);
                *status = format!("Started: {}", job.label);
                self.pipeline = Some(job);
            }
            Ok(None) => {
                *status = "No correction needed (drift < 0.001%)".to_string();
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    /// Launch native audio cleanup (hum notch + noisered) with the given
    /// params. Output is written beside the input, keeping `library.rs`'s VD
    /// classification intact — see `audio_cleanup::output_path`'s doc comment.
    fn launch_audio_cleanup(
        &mut self,
        input: PathBuf,
        params: &crate::audio_cleanup::Params,
        cfg: &Config,
        status: &mut String,
    ) {
        let output = crate::audio_cleanup::output_path(&input);
        let name = input
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("input")
            .to_string();
        match crate::audio_cleanup::launch(
            &input,
            &output,
            cfg,
            format!("Audio Cleanup {name}"),
            params,
        ) {
            Ok(mut job) => {
                job.output_path = Some(output);
                *status = format!("Started: {}", job.label);
                self.pipeline = Some(job);
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    fn upscale_output(input: &std::path::Path, cfg: &Config) -> PathBuf {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        let clean = stem.strip_suffix(".viewer").unwrap_or(stem);
        cfg.viewer_dir().join(format!("{clean}.upscale.mkv"))
    }

    fn upscale_segments_dir(input: &std::path::Path, cfg: &Config) -> PathBuf {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        cfg.upscale_work_root().join(stem).join("segments")
    }

    /// Delete the work dir for `input` if its saved MODEL differs from `current_model`.
    fn clear_stale_work_dir(input: &std::path::Path, cfg: &Config, current_model: Option<&str>) {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        let work_dir = cfg.upscale_work_root().join(stem);
        let config_file = work_dir.join("run_config.txt");
        let Ok(contents) = std::fs::read_to_string(&config_file) else {
            return;
        };
        let saved_model = contents.lines().find_map(|l| l.strip_prefix("MODEL="));
        if saved_model != current_model {
            let _ = std::fs::remove_dir_all(&work_dir);
        }
    }

    fn launch_upscale(
        &mut self,
        label: String,
        variant: Variant,
        input: PathBuf,
        output: PathBuf,
        cfg: &Config,
        status: &mut String,
    ) {
        let seg_dir = Self::upscale_segments_dir(&input, cfg);
        let seg_secs = self.settings.segment_secs;

        // A job from an earlier vhs-gui process (e.g. one that exited when a
        // display-wake event disrupted the GL surface — see on_exit's doc
        // comment) may still be running against this exact work dir. Reattach
        // to it instead of spawning a second Real-ESRGAN process on top of the
        // same checkpoint segments, which would corrupt them.
        if let Some(work_dir) = seg_dir.parent()
            && let Some(pgid) = PipelineJob::check_lock(work_dir)
        {
            let running_secs = PipelineJob::running_segment_secs(work_dir).unwrap_or(seg_secs);
            let job =
                PipelineJob::attach_running(pgid, label, &input, seg_dir, output, running_secs);
            *status = format!("Reattached to already-running upscale (pid {pgid})");
            self.last_preview_at = None;
            self.last_preview_frames = 0;
            self.preview_textures = None;
            self.pipeline = Some(job);
            self.upscale_started = true;
            return;
        }

        let crop = self.letterbox.crop_for(&input);
        // Guaranteed correct regardless of whether the settings panel was
        // open (see sync_scale_to_model's doc comment) — a stale scale here
        // would both upscale at the wrong factor and corrupt the resume
        // fingerprint match against any existing checkpoint for this file.
        self.settings.sync_scale_to_model();

        Self::clear_stale_work_dir(&input, cfg, self.settings.selected_model());

        // The native engine, run as a detached worker, for every variant.
        let mut spec =
            self.settings
                .to_job_spec(variant, &input, &output, &cfg.upscale_work_root());
        spec.crop = crop;
        if has_square_pixels(&input, cfg) {
            spec.target_dar.clear();
        }
        match crate::upscale::start_worker(&spec, label, &cfg.log_dir()) {
            Ok(job) => {
                let job = job.with_upscale_tracking(seg_dir, output, seg_secs);
                *status = format!("Started: {}", job.label);
                if let Some(c) = crop {
                    *status = format!("{status} (cropped to {}x{})", c.w, c.h);
                }
                self.last_preview_at = None;
                self.last_preview_frames = 0;
                self.preview_textures = None;
                self.pipeline = Some(job);
                self.upscale_started = true;
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // The Library's action region
    // -----------------------------------------------------------------------

    /// The action region under the file list: the groups of actions for the selected file, or one
    /// inline panel that replaces them. Drawn by the caller in a bottom panel, so the list above
    /// never moves.
    pub fn show_actions(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        cfg: &Config,
        status: &mut String,
    ) {
        let Some(entry) = self.library.selected_entry().cloned() else {
            self.inline = Inline::None;
            return;
        };
        // An inline panel belongs to one file: selecting another closes it.
        if self.inline.path().is_some_and(|p| p != &entry.path) {
            self.inline = Inline::None;
        }
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        if matches!(self.inline, Inline::None) {
            self.show_action_groups(ui, &entry, cfg, status);
        } else {
            self.show_inline(ui, ctx, &entry, cfg, status);
        }
    }

    fn show_action_groups(
        &mut self,
        ui: &mut egui::Ui,
        entry: &crate::library::LibraryEntry,
        cfg: &Config,
        status: &mut String,
    ) {
        let busy = self.pipeline.is_some();
        if let Some(job) = &self.pipeline {
            ui.label(
                egui::RichText::new(format!(
                    "{} Actions return when {} ends",
                    icons::TIME,
                    job.label
                ))
                .size(11.0)
                .color(theme::color::WARNING),
            );
        }
        let groups = super::actions::groups(&entry.kind);
        let picked = ui
            .add_enabled_ui(!busy, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                super::actions::show_groups(ui, &groups)
            })
            .inner;

        // A crop already found for this file: one line that reopens the panel.
        if let Some(crop) = self.letterbox.crop_for(&entry.path) {
            let line = egui::RichText::new(format!(
                "{} {}x{} · crop on",
                icons::LETTERBOX,
                crop.w,
                crop.h
            ))
            .monospace()
            .size(11.0)
            .color(theme::color::ACCENT);
            if ui
                .add(egui::Label::new(line).sense(egui::Sense::click()))
                .clicked()
            {
                self.inline = Inline::Letterbox(entry.path.clone());
            }
        }

        if let Some(id) = picked {
            self.run_action(id, entry, cfg, status);
        }
    }

    /// Start (or open the panel for) the chosen action. Every launch is the call the old buttons
    /// made, with the same job label.
    fn run_action(
        &mut self,
        id: super::actions::ActionId,
        entry: &crate::library::LibraryEntry,
        cfg: &Config,
        status: &mut String,
    ) {
        use super::actions::ActionId as A;
        let path = entry.path.clone();
        match id {
            A::Denoise => self.launch_denoise(path, None, cfg, status),
            A::DenoiseQtgmc => {
                if entry.name.ends_with("_STABLE.mkv") {
                    // Already denoised: vhs_process.sh skipped straight to QTGMC.
                    self.launch_vs_op(VsOp::Qtgmc, path, cfg, status);
                } else {
                    self.launch_denoise(path, Some(VsOp::Qtgmc), cfg, status);
                }
            }
            A::Qtgmc => self.launch_vs_op(VsOp::Qtgmc, path, cfg, status),
            A::Ivtc => self.launch_vs_op(VsOp::Ivtc, path, cfg, status),
            A::IvtcDecomb => self.launch_vs_op(VsOp::IvtcDecombed, path, cfg, status),
            A::FieldAlign => self.launch_vs_op(VsOp::FieldAlign, path, cfg, status),
            A::Vdecimate => self.launch_vs_op(VsOp::Vdecimate, path, cfg, status),
            A::FixSync => self.launch_fix_sync(path, cfg, status),
            A::ViewerEncode => self.launch_viewer_encode(path, cfg, status),
            A::Upscale(variant, verb) => {
                let out = Self::upscale_output(&path, cfg);
                self.launch_upscale(
                    format!("{verb} {}", entry.name),
                    variant,
                    path,
                    out,
                    cfg,
                    status,
                );
            }
            A::DetectLetterbox => {
                self.letterbox.start_if_needed(&path);
                self.inline = Inline::Letterbox(path);
            }
            A::AudioCleanup => {
                self.inline = Inline::AudioCleanup(path, crate::audio_cleanup::Params::default());
            }
            A::Rename => {
                let suggestion = suggest_viewer_name(&path);
                self.inline = Inline::Rename(path, suggestion);
            }
        }
    }

    fn file_name_line(ui: &mut egui::Ui, name: &str) {
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

    fn show_inline(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        entry: &crate::library::LibraryEntry,
        cfg: &Config,
        status: &mut String,
    ) {
        // Take the panel out so its state can be edited while `self` is used for the actions.
        let mut inline = std::mem::replace(&mut self.inline, Inline::None);
        let keep = match &mut inline {
            Inline::None => false,
            Inline::Trash(path) => self.trash_panel(ui, path, entry, cfg, status),
            Inline::Rename(_, edit) => self.rename_panel(ui, edit, entry, cfg, status),
            Inline::AudioCleanup(_, params) => {
                self.audio_cleanup_panel(ui, ctx, params, entry, cfg, status)
            }
            Inline::Letterbox(path) => {
                let done = self.letterbox.show_panel(ui, ctx, path, status);
                !done
            }
        };
        if keep {
            self.inline = inline;
        }
    }

    /// Move to Trash?: No (first, takes focus) and Yes, move it. Returns true to stay open.
    fn trash_panel(
        &mut self,
        ui: &mut egui::Ui,
        path: &std::path::Path,
        entry: &crate::library::LibraryEntry,
        cfg: &Config,
        status: &mut String,
    ) -> bool {
        if super::actions::inline_header(ui, "Move to Trash?", theme::color::DANGER) {
            return false;
        }
        Self::file_name_line(ui, &entry.name);
        ui.label(
            egui::RichText::new("The file goes to the desktop trash.")
                .size(11.5)
                .color(theme::color::TEXT_WEAK),
        );
        let (mut no, mut yes) = (false, false);
        ui.columns(2, |cols| {
            let keep = cols[0].add_sized(
                [cols[0].available_width(), theme::size::CONTROL_HEIGHT + 2.0],
                egui::Button::new("No"),
            );
            if !keep.has_focus() {
                keep.request_focus();
            }
            no = keep.clicked() || cols[0].input(|i| i.key_pressed(egui::Key::Escape));
            cols[1].spacing_mut().button_padding.x = 4.0;
            let trash = theme::danger_solid_button(format!("{} Yes, move it", icons::TRASH))
                .wrap_mode(egui::TextWrapMode::Truncate);
            yes = cols[1]
                .add_sized(
                    [cols[1].available_width(), theme::size::CONTROL_HEIGHT + 2.0],
                    trash,
                )
                .clicked();
        });
        if yes {
            if let Err(e) = trash::delete(path) {
                *status = format!("Trash failed: {e}");
            } else {
                *status = format!("Trashed {}", entry.name);
            }
            self.library.refresh(cfg);
            return false;
        }
        !no
    }

    /// Rename (Viewer files): the field is pre-filled from `suggest_viewer_name`.
    fn rename_panel(
        &mut self,
        ui: &mut egui::Ui,
        edit: &mut String,
        entry: &crate::library::LibraryEntry,
        cfg: &Config,
        status: &mut String,
    ) -> bool {
        if super::actions::inline_header(ui, "Rename", theme::color::TEXT_STRONG) {
            return false;
        }
        Self::file_name_line(ui, &entry.name);
        ui.label(
            egui::RichText::new("Rename to:")
                .size(11.5)
                .color(theme::color::TEXT_WEAK),
        );
        ui.add(
            egui::TextEdit::singleline(edit)
                .desired_width(f32::INFINITY)
                .hint_text("filename.mkv"),
        );
        let (mut ok, mut cancel) = (false, false);
        ui.columns(2, |cols| {
            ok = cols[0]
                .add_sized(
                    [cols[0].available_width(), theme::size::CONTROL_HEIGHT + 2.0],
                    theme::primary_button("OK"),
                )
                .clicked();
            cancel = cols[1]
                .add_sized(
                    [cols[1].available_width(), theme::size::CONTROL_HEIGHT + 2.0],
                    egui::Button::new("Cancel"),
                )
                .clicked();
        });
        if cancel {
            return false;
        }
        if ok {
            let new_name = edit.trim().to_owned();
            if !new_name.is_empty() {
                let new_name = if new_name.ends_with(".mkv") {
                    new_name
                } else {
                    format!("{new_name}.mkv")
                };
                let new_path = entry
                    .path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(&new_name);
                if new_path.exists() {
                    *status = format!("Rename failed: {new_name} already exists");
                } else {
                    match std::fs::rename(&entry.path, &new_path) {
                        Ok(()) => {
                            *status = format!("Renamed to {new_name}");
                            self.library.refresh(cfg);
                            // Keep the renamed file selected so its actions stay in view.
                            self.library.selected =
                                self.library.entries.iter().position(|e| e.path == new_path);
                        }
                        Err(e) => *status = format!("Rename failed: {e}"),
                    }
                }
            }
            return false;
        }
        true
    }

    /// Audio Cleanup: the form, with Find Quiet Spot directly under the two fields it fills.
    fn audio_cleanup_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        params: &mut crate::audio_cleanup::Params,
        entry: &crate::library::LibraryEntry,
        cfg: &Config,
        status: &mut String,
    ) -> bool {
        if super::actions::inline_header(ui, "Audio Cleanup", theme::color::TEXT_STRONG) {
            return false;
        }
        // Compact: smaller text and tighter rows, so the form does not widen the side panel.
        ui.style_mut().override_font_id =
            Some(egui::FontId::new(11.5, egui::FontFamily::Proportional));
        ui.spacing_mut().item_spacing.y = 4.0;
        let field = |ui: &mut egui::Ui, drag: egui::DragValue<'_>| {
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.x = 62.0;
                ui.add(drag)
            });
        };
        egui::Grid::new("audio_cleanup_sample")
            .num_columns(2)
            .spacing([6.0, 3.0])
            .show(ui, |ui| {
                ui.label("Noise sample start (s)");
                field(
                    ui,
                    egui::DragValue::new(&mut params.noise_start_secs)
                        .range(0.0..=f64::MAX)
                        .speed(0.1),
                );
                ui.end_row();
                ui.label("Noise sample length (s)");
                field(
                    ui,
                    egui::DragValue::new(&mut params.noise_len_secs)
                        .range(0.1..=10.0)
                        .speed(0.1),
                );
                ui.end_row();
            });

        // "Find Quiet Spot": scans the file with ffmpeg silencedetect and fills the two fields
        // above, instead of the user hand-scanning timestamps.
        let scan_running = self
            .quiet_spot_scan
            .as_ref()
            .is_some_and(|(p, _)| p == &entry.path);
        if scan_running {
            let (_, scan) = self.quiet_spot_scan.as_ref().unwrap();
            match scan.poll() {
                None => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(theme::color::ACCENT));
                        ui.label(
                            egui::RichText::new(format!("Scanning… {}", scan.elapsed_str()))
                                .size(11.5)
                                .color(theme::color::TEXT_WEAK),
                        );
                        if ui.button("Cancel").clicked() {
                            scan.cancel();
                        }
                    });
                    ctx.request_repaint_after(std::time::Duration::from_secs(1));
                }
                Some(result) => {
                    self.quiet_spot_scan = None;
                    match result {
                        Ok(Some(spot)) => {
                            params.noise_start_secs = spot.start_secs;
                            params.noise_len_secs = spot.len_secs;
                            let m = (spot.start_secs / 60.0) as u64;
                            let s = spot.start_secs % 60.0;
                            *status = format!(
                                "Found quiet spot: {m}:{s:04.1} \
                                 ({:.1}s stretch, {:.1}dB) -- filled in above",
                                spot.gap_secs, spot.rms_db
                            );
                        }
                        Ok(None) => {
                            *status = "No quiet stretch found (tried -35dB) \
                                       -- try a different noise start manually."
                                .to_string();
                        }
                        Err(e) => *status = format!("Quiet-spot scan failed: {e}"),
                    }
                }
            }
        } else {
            let find = egui::Button::new(format!("{} Find Quiet Spot", icons::SEARCH));
            if ui
                .add_sized([ui.available_width(), theme::size::CONTROL_HEIGHT], find)
                .clicked()
            {
                self.quiet_spot_scan = Some((
                    entry.path.clone(),
                    crate::audio_cleanup::QuietSpotScan::start(&entry.path),
                ));
            }
        }

        egui::Grid::new("audio_cleanup_rest")
            .num_columns(2)
            .spacing([6.0, 3.0])
            .show(ui, |ui| {
                ui.label("Noise reduction amount");
                field(
                    ui,
                    egui::DragValue::new(&mut params.nr_amount)
                        .range(0.0..=1.0)
                        .speed(0.01),
                );
                ui.end_row();
                ui.label("Hum notch (60Hz)");
                ui.checkbox(&mut params.hum_enable, "");
                ui.end_row();
                ui.label("Loudness target (LUFS)");
                field(
                    ui,
                    egui::DragValue::new(&mut params.loudnorm_target_i)
                        .range(-30.0..=-6.0)
                        .speed(0.5),
                );
                ui.end_row();
            });
        ui.label(
            egui::RichText::new("-16 suits spoken word; try -20 for wider-range music.")
                .size(10.5)
                .color(theme::color::TEXT_WEAK),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} Point the sample at a real quiet spot on this tape. A wrong window makes \
                 cleanup worse.",
                icons::WARNING
            ))
            .size(10.5)
            .color(theme::color::WARNING),
        );

        let (mut run, mut cancel) = (false, false);
        ui.columns(2, |cols| {
            run = cols[0]
                .add_sized(
                    [cols[0].available_width(), theme::size::CONTROL_HEIGHT + 2.0],
                    theme::primary_button(format!("{} Run", icons::PLAY)),
                )
                .clicked();
            cancel = cols[1]
                .add_sized(
                    [cols[1].available_width(), theme::size::CONTROL_HEIGHT + 2.0],
                    egui::Button::new("Cancel"),
                )
                .clicked();
        });
        if run {
            self.launch_audio_cleanup(entry.path.clone(), params, cfg, status);
            return false;
        }
        !cancel
    }
}

// -----------------------------------------------------------------------
// Free helpers
// -----------------------------------------------------------------------

/// What to start when a job finishes: the chained VapourSynth op, on the finished job's output.
/// Only after success: a failed or cancelled Denoise must never start QTGMC on a partial file.
fn chained_next(
    failed: bool,
    chained: Option<VsOp>,
    output: Option<PathBuf>,
) -> Option<(VsOp, PathBuf)> {
    match (failed, chained, output) {
        (false, Some(op), Some(path)) => Some((op, path)),
        _ => None,
    }
}

/// True if a finished job should be reported as failed: a non-zero exit
/// (native jobs only — bash jobs don't check exit codes today), or a
/// tracked output_path that never actually appeared. Untracked (None)
/// output_path isn't penalized — it was never checked before.
fn job_failed(exit_ok: Option<bool>, output_path: &Option<PathBuf>) -> bool {
    let exit_failed = exit_ok == Some(false);
    let output_missing = output_path
        .as_deref()
        .map(|p| !p.is_file())
        .unwrap_or(false);
    exit_failed || output_missing
}

/// A viewer copy is already scaled to square pixels (Viewer Encode's `setsar=1`), so its picture
/// shape is its width and height. Forcing the NTSC 4:3 display aspect on it squashed a 640x274
/// letterboxed copy to 366x274. Masters keep the 4:3, since their 720x480 pixels are not square.
fn has_square_pixels(input: &std::path::Path, cfg: &Config) -> bool {
    input.starts_with(cfg.viewer_dir())
}

fn cleanup_upscale_work_dir(
    output_path: &Option<PathBuf>,
    segments_dir: &Option<PathBuf>,
) -> Option<String> {
    let out = output_path.as_ref()?;
    if !out.exists() {
        return None;
    }
    let work_dir = segments_dir.as_ref()?.parent()?;
    match std::fs::remove_dir_all(work_dir) {
        Ok(()) => Some("work dir cleaned up".into()),
        Err(e) => Some(format!("cleanup failed: {e}")),
    }
}

pub(super) fn load_jpeg_as_egui_image(path: &std::path::Path) -> Option<egui::ColorImage> {
    let img = image::open(path).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        img.as_raw(),
    ))
}

fn latest_jpg_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jpg"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries.last().map(|e| e.path())
}

fn title_words(s: &str) -> String {
    const ACRONYMS: &[&str] = &["VHS", "TV", "BBC", "DVD", "CD", "UK", "US", "USA"];
    s.replace('_', " ")
        .split_whitespace()
        .map(|w| {
            let up = w.to_uppercase();
            if ACRONYMS.contains(&up.as_str()) {
                up
            } else {
                let mut chars = w.chars();
                match chars.next() {
                    None => String::new(),
                    Some(f) => {
                        let head: String = f.to_uppercase().collect();
                        head + &chars.as_str().to_lowercase()
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn suggest_viewer_name(path: &std::path::Path) -> String {
    let name = match path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n,
        None => return String::new(),
    };
    let stem = name.strip_suffix(".mkv").unwrap_or(name);
    let stem = stem.strip_suffix(".upscale").unwrap_or(stem);
    let stem = stem.strip_suffix(".viewer").unwrap_or(stem);
    let stem = stem.strip_suffix("_VD").unwrap_or(stem);
    let stem = stem.strip_prefix("EDIT_MASTER-").unwrap_or(stem);
    if stem.is_empty() {
        return name.to_owned();
    }
    if let Some(dash) = stem.find('-') {
        let type_part = &stem[..dash];
        let title_part = &stem[dash + 1..];
        if !type_part.is_empty() && !title_part.is_empty() {
            return format!(
                "{} \u{2014} {}.mkv",
                title_words(type_part),
                title_words(title_part),
            );
        }
    }
    // No type/title dash to split on (e.g. a tape whose title itself has no
    // internal "-", so it's not "TYPE-TITLE") — still title-case the whole
    // stem instead of falling back to the untouched original name, which
    // would silently offer the current filename as its own "suggestion" and
    // make a same-name rename fail with a false "already exists".
    format!("{}.mkv", title_words(stem))
}

#[cfg(test)]
mod job_failed_tests {
    use super::{chained_next, job_failed};
    use crate::vs_ops::VsOp;
    use std::path::PathBuf;

    #[test]
    fn chained_op_starts_only_after_a_successful_job() {
        let out = || Some(PathBuf::from("/s/clip_STABLE.mkv"));
        assert_eq!(
            chained_next(false, Some(VsOp::Qtgmc), out()),
            Some((VsOp::Qtgmc, PathBuf::from("/s/clip_STABLE.mkv")))
        );
        // A failed or cancelled job never starts the next stage.
        assert_eq!(chained_next(true, Some(VsOp::Qtgmc), out()), None);
        // Nothing chained, or nothing to chain onto.
        assert_eq!(chained_next(false, None, out()), None);
        assert_eq!(chained_next(false, Some(VsOp::Qtgmc), None), None);
    }

    #[test]
    fn nonzero_exit_is_failure_even_with_output_present() {
        let f = std::env::temp_dir().join("vhs_gui_job_failed_test_present.mkv");
        std::fs::write(&f, b"x").unwrap();
        assert!(job_failed(Some(false), &Some(f.clone())));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn missing_output_is_failure_even_with_clean_exit() {
        let f = PathBuf::from("/nonexistent/vhs_gui_job_failed_test.mkv");
        assert!(job_failed(Some(true), &Some(f)));
    }

    #[test]
    fn clean_exit_with_existing_output_is_success() {
        let f = std::env::temp_dir().join("vhs_gui_job_failed_test_ok.mkv");
        std::fs::write(&f, b"x").unwrap();
        assert!(!job_failed(Some(true), &Some(f.clone())));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn untracked_output_path_is_not_penalized() {
        // None output_path means it was never checked before this job type
        // supported tracking — don't treat that as failure on its own.
        assert!(!job_failed(None, &None));
    }
}

#[cfg(test)]
mod suggest_viewer_name_tests {
    use super::suggest_viewer_name;
    use std::path::Path;

    #[test]
    fn tagged_name_with_type_title_dash_is_humanized() {
        let p = Path::new("EDIT_MASTER-VHS_TRAILER-THE_GREAT_MOUSE_DETECTIVE_VD.upscale.mkv");
        assert_eq!(
            suggest_viewer_name(p),
            "VHS Trailer — The Great Mouse Detective.mkv"
        );
    }

    /// Regression test: a tape title with no internal "-" (not a
    /// "TYPE-TITLE" pattern) previously fell back to the untouched original
    /// filename as its own "suggestion" — if that file was already
    /// untagged, the suggestion was byte-identical to the current name, so
    /// accepting it without editing failed with a false "already exists".
    #[test]
    fn dashless_untagged_name_is_still_title_cased_and_differs_from_input() {
        let p = Path::new("TED_BARYLUKS_GROCERY.mkv");
        let suggestion = suggest_viewer_name(p);
        assert_eq!(suggestion, "Ted Baryluks Grocery.mkv");
        assert_ne!(suggestion, "TED_BARYLUKS_GROCERY.mkv");
    }

    #[test]
    fn dashless_tagged_name_strips_tags_and_title_cases() {
        let p = Path::new("EDIT_MASTER-TED_BARYLUKS_GROCERY_VD.upscale.mkv");
        assert_eq!(suggest_viewer_name(p), "Ted Baryluks Grocery.mkv");
    }
}

#[cfg(test)]
mod stale_work_dir_tests {
    use super::UpscalePanel;
    use crate::config::Config;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn setup(name: &str, saved: Option<&str>) -> (Config, PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("vhs-gui-stale-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let work = root.join("clip");
        fs::create_dir_all(work.join("segments")).unwrap();
        fs::write(work.join("segments/seg_000.mp4"), b"data").unwrap();
        if let Some(model) = saved {
            fs::write(
                work.join("run_config.txt"),
                format!("MODEL={model}\nCRF=21"),
            )
            .unwrap();
        }
        let cfg = Config {
            work_root: root.clone(),
            ..Config::default()
        };
        (cfg, root, work)
    }

    fn run(cfg: &Config, model: Option<&str>) {
        UpscalePanel::clear_stale_work_dir(Path::new("/in/clip.mkv"), cfg, model);
    }

    #[test]
    fn a_different_model_discards_the_old_checkpoints() {
        let (cfg, root, work) = setup("differs", Some("realesrgan-x4plus"));
        run(&cfg, Some("realesrgan-x4plus-anime"));
        assert!(!work.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_same_model_keeps_them_for_resuming() {
        let (cfg, root, work) = setup("same", Some("realesrgan-x4plus"));
        run(&cfg, Some("realesrgan-x4plus"));
        assert!(work.join("segments/seg_000.mp4").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_work_dir_without_a_saved_config_is_left_alone() {
        let (cfg, root, work) = setup("noconfig", None);
        run(&cfg, Some("realesrgan-x4plus"));
        assert!(work.join("segments/seg_000.mp4").exists());
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod square_pixel_tests {
    #[test]
    fn only_viewer_copies_are_treated_as_square_pixel() {
        let cfg = crate::config::Config::default();
        assert!(super::has_square_pixels(
            &cfg.viewer_dir().join("EDIT_MASTER-X_VD.viewer.mkv"),
            &cfg
        ));
        assert!(!super::has_square_pixels(
            &cfg.stabilized_dir().join("EDIT_MASTER-X_VD.mkv"),
            &cfg
        ));
        assert!(!super::has_square_pixels(
            &cfg.archival_dir().join("VHS_ARCHIVAL_x.mkv"),
            &cfg
        ));
    }
}
