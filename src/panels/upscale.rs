use std::path::PathBuf;

use crate::config::Config;
use crate::library::{FileKind, Library};
use crate::mpv_view::{MpvView, Source};
use crate::pipeline::PipelineJob;
use crate::settings::{
    Backend, BrightnessPreset, CrushPreset, UpscaleSettings, preset_models_dirs,
};
use crate::vs_ops::VsOp;

struct UpscalePreviewTextures {
    orig: egui::TextureHandle,
    upscaled: egui::TextureHandle,
    segment: u64,
    total_segments: u64,
    frame: u64,
    segment_frames: u64,
}

pub struct UpscalePanel {
    pub library: Library,
    pub settings: UpscaleSettings,
    pub settings_panel_open: bool,
    pipeline: Option<PipelineJob>,
    confirm_delete: Option<PathBuf>,
    rename_state: Option<(PathBuf, String)>,
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
            confirm_delete: None,
            rename_state: None,
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
                if ui.small_button("◀").on_hover_text("Close panel").clicked() {
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
                    ui.radio_value(&mut self.settings.backend, Backend::Rocm, "ROCm");
                    ui.radio_value(&mut self.settings.backend, Backend::Vulkan, "Vulkan");
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
                            .small_button("⟳")
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
                        if let Some(name) = self.settings.selected_model()
                            && let Some(s) = crate::settings::infer_scale(name)
                        {
                            self.settings.internal_scale = s;
                        }
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
                    if self.settings.internal_scale != s {
                        self.settings.internal_scale = s;
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

                // --- Denoise ---
                ui.label("Denoise");
                if ui.checkbox(&mut self.settings.denoise, "").changed() {
                    changed = true;
                }
                ui.end_row();
            });
        changed
    }

    pub fn refresh_library(&mut self, cfg: &Config) {
        self.library.refresh(cfg);
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

    /// Draw the toolbar section for the Upscale view.
    /// Shows job label, elapsed time, progress, and pause/stop/cancel controls.
    /// Does nothing when no job is running.
    pub fn toolbar_section(&mut self, ui: &mut egui::Ui, status: &mut String) {
        let job = match self.pipeline.as_mut() {
            Some(j) => j,
            None => return,
        };

        ui.label(egui::RichText::new(&job.label).small());
        ui.label(
            egui::RichText::new(job.elapsed_str())
                .monospace()
                .small()
                .weak(),
        );

        // Progress indicator.
        if job.is_upscale {
            if job.total_segments > 0 {
                let seg_label =
                    format!("seg {}/{}", job.completed_segments + 1, job.total_segments);
                ui.label(egui::RichText::new(seg_label).small());
            }
            if let Some(p) = job.segment_progress() {
                ui.add(
                    egui::ProgressBar::new(p)
                        .desired_width(80.0)
                        .show_percentage(),
                );
            }
        } else if let Some(p) = job.progress() {
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(120.0)
                    .show_percentage(),
            );
        }

        ui.separator();

        // Pause / Resume.
        let (pause_label, pause_tip) = if job.paused {
            ("▶", "Resume")
        } else {
            ("⏸", "Pause")
        };
        if ui.button(pause_label).on_hover_text(pause_tip).clicked() {
            job.toggle_pause();
        }

        // Stop after segment (upscale only, when not already requested).
        if job.is_upscale {
            if job.stopping_after_segment() {
                ui.label(egui::RichText::new("stopping…").italics().small().weak());
            } else {
                if ui.button("Stop after seg").clicked() {
                    job.request_stop_after_segment();
                    *status = "Stopping after current segment…".into();
                }
            }
        }

        // Cancel — sends SIGINT immediately; poll() detects exit next frame.
        if ui
            .button("Cancel")
            .on_hover_text("Terminate immediately")
            .clicked()
        {
            job.cancel();
            *status = "Cancelling…".into();
        }
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
            let finish_status = if job.is_upscale {
                cleanup_upscale_work_dir(&job.output_path, &job.segments_dir)
                    .map(|msg| format!("{} finished — {msg}", job.label))
                    .unwrap_or_else(|| format!("{} finished", job.label))
            } else {
                // exit_ok is only meaningful for native jobs (bash jobs never
                // check exit codes today); a tracked output_path that's
                // missing is also treated as failure. Untracked (None)
                // output_path isn't penalized — it was never checked before.
                let exit_failed = job.exit_ok == Some(false);
                let output_missing = job
                    .output_path
                    .as_deref()
                    .map(|p| !p.is_file())
                    .unwrap_or(false);
                if exit_failed || output_missing {
                    format!("{} FAILED — see log", job.label)
                } else {
                    format!("{} finished", job.label)
                }
            };
            *status = finish_status;
            self.last_preview_at = None;
            self.last_preview_frames = 0;
            self.preview_textures = None;
            self.pipeline = None;
            self.library.refresh(cfg);
            if let Some(p) = reselect_path {
                self.library.selected = self.library.entries.iter().position(|e| e.path == p);
            }
        }
    }

    /// Draw the library list + file action panel inside a caller-supplied scroll area.
    pub fn show_sidebar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        mpv: &mut MpvView,
        cfg: &Config,
        status: &mut String,
    ) {
        if let Some(entry) = self.library.show(ui) {
            *status = format!("Opening: {}", entry.name);
            mpv.open(&Source::File(entry.path));
        }
        if self.library.selected_entry().is_some() {
            self.file_actions_panel(ui, ctx, cfg, status);
        }
    }

    /// Draw the central-panel content when an upscale job is active.
    pub fn show_central(&self, ui: &mut egui::Ui) {
        // Progress bars at top, always visible (even before the first preview frame).
        if let Some(ref job) = self.pipeline {
            if job.is_upscale {
                let total_p = job.total_progress().unwrap_or(0.0);
                let seg_p = job.segment_progress().unwrap_or(0.0);
                let total_text = if job.total_segments > 0 {
                    format!("{}/{} segs", job.completed_segments, job.total_segments)
                } else {
                    "…".into()
                };
                let seg_text = if job.segment_frames > 0 {
                    format!("{}/{} frames", job.upscaled_frames, job.segment_frames)
                } else {
                    "…".into()
                };
                ui.add(
                    egui::ProgressBar::new(total_p)
                        .desired_width(f32::INFINITY)
                        .text(total_text),
                );
                ui.add(
                    egui::ProgressBar::new(seg_p)
                        .desired_width(f32::INFINITY)
                        .text(seg_text),
                );
                ui.add_space(4.0);
            } else if let Some(p) = job.progress() {
                ui.add(
                    egui::ProgressBar::new(p)
                        .desired_width(f32::INFINITY)
                        .show_percentage(),
                );
                ui.add_space(4.0);
            }
        }

        if let Some(ref textures) = self.preview_textures {
            // clip_rect() is always finite; available_size().x and max_rect().width() can be inf.
            let avail_w = ui.clip_rect().width();
            let avail_h = ui.available_size().y;
            let label_h = 18.0;
            let gap = 6.0;
            let item_sp = ui.spacing().item_spacing.x;
            // Total horizontal overhead: explicit gap + two item-spacings flanking it.
            let panel_w = ((avail_w - gap - item_sp * 2.0) / 2.0).floor();
            let panel_h = (panel_w * 3.0 / 4.0).min(avail_h - label_h - 4.0);

            let seg_label = format!(
                "Seg {} / {}  ·  Frame {} / {}",
                textures.segment, textures.total_segments, textures.frame, textures.segment_frames,
            );

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(panel_w);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Original  720×480").small().weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(&seg_label).small().weak());
                        });
                    });
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::from_handle(&textures.orig))
                            .fit_to_exact_size(egui::vec2(panel_w, panel_h)),
                    );
                });
                ui.add_space(gap);
                ui.vertical(|ui| {
                    ui.set_max_width(panel_w);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Upscaled  {}×",
                                self.settings.internal_scale
                            ))
                            .small()
                            .weak(),
                        );
                    });
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::from_handle(&textures.upscaled))
                            .fit_to_exact_size(egui::vec2(panel_w, panel_h)),
                    );
                });
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Upscaling…\nPreview frames will appear shortly").weak(),
                );
            });
        }
    }

    // -----------------------------------------------------------------------
    // Launch helpers
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn launch_pipeline(
        &mut self,
        label: String,
        script: PathBuf,
        input: PathBuf,
        envs: &[(&str, &str)],
        extra_args: &[&str],
        cfg: &Config,
        status: &mut String,
    ) {
        match PipelineJob::start(label, &script, &input, envs, extra_args, &cfg.log_dir()) {
            Ok(job) => {
                *status = format!("Started: {}", job.label);
                self.pipeline = Some(job);
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
        script: PathBuf,
        input: PathBuf,
        output: PathBuf,
        cfg: &Config,
        status: &mut String,
    ) {
        Self::clear_stale_work_dir(&input, cfg, self.settings.selected_model());

        let seg_dir = Self::upscale_segments_dir(&input, cfg);
        let seg_secs = self.settings.segment_secs;

        let (mut owned_envs, owned_args) = self.settings.to_launch(&output);
        // WORK_ROOT must be passed explicitly: the bash script otherwise
        // resolves its own default independently of cfg.upscale_work_root(),
        // which is what Rust uses for segment-checkpoint bookkeeping above —
        // without this they can silently point at different directories.
        owned_envs.push((
            "WORK_ROOT".into(),
            cfg.upscale_work_root().to_string_lossy().into_owned(),
        ));
        let env_refs: Vec<(&str, &str)> = owned_envs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let arg_refs: Vec<&str> = owned_args.iter().map(|s| s.as_str()).collect();

        match PipelineJob::start(label, &script, &input, &env_refs, &arg_refs, &cfg.log_dir()) {
            Ok(job) => {
                let job = job.with_upscale_tracking(seg_dir, output, seg_secs);
                *status = format!("Started: {}", job.label);
                self.last_preview_at = None;
                self.last_preview_frames = 0;
                self.preview_textures = None;
                self.pipeline = Some(job);
            }
            Err(e) => *status = format!("Failed to start job: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // File action panel
    // -----------------------------------------------------------------------

    fn file_actions_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        cfg: &Config,
        status: &mut String,
    ) {
        let entry = match self.library.selected_entry() {
            Some(e) => e.clone(),
            None => return,
        };

        ui.separator();
        ui.label(egui::RichText::new(&entry.name).small().weak());

        let busy =
            self.pipeline.is_some() || self.confirm_delete.is_some() || self.rename_state.is_some();

        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                match entry.kind {
                    FileKind::Archival => {
                        if ui.button("Denoise").clicked() {
                            self.launch_pipeline(
                                format!("Denoise {}", entry.name),
                                cfg.denoise_script(),
                                entry.path.clone(),
                                &[],
                                &[],
                                cfg,
                                status,
                            );
                        }
                        if ui.button("Denoise+QTGMC").clicked() {
                            // vhs_process.sh resolves its own DENOISE_SH/VPY via
                            // SCRIPTS_DIR internally — pass it explicitly so a
                            // GUI-configured scripts_dir actually reaches it,
                            // rather than the script falling back to its own
                            // hardcoded default.
                            let scripts_dir = cfg.scripts_dir.to_string_lossy().into_owned();
                            self.launch_pipeline(
                                format!("Denoise+QTGMC {}", entry.name),
                                cfg.process_script(),
                                entry.path.clone(),
                                &[("NO_LAUNCH", "1"), ("SCRIPTS_DIR", &scripts_dir)],
                                &[],
                                cfg,
                                status,
                            );
                        }
                    }
                    FileKind::Stabilized => {
                        if ui.button("QTGMC").clicked() {
                            self.launch_vs_op(VsOp::Qtgmc, entry.path.clone(), cfg, status);
                        }
                        if ui.button("IVTC").clicked() {
                            self.launch_vs_op(VsOp::Ivtc, entry.path.clone(), cfg, status);
                        }
                        if ui.button("IVTC+Decomb").clicked() {
                            self.launch_vs_op(VsOp::IvtcDecombed, entry.path.clone(), cfg, status);
                        }
                        if ui.button("Field Align").clicked() {
                            self.launch_vs_op(VsOp::FieldAlign, entry.path.clone(), cfg, status);
                        }
                        if ui.button("Fix A/V Sync").clicked() {
                            self.launch_fix_sync(entry.path.clone(), cfg, status);
                        }
                    }
                    FileKind::EditMaster => {
                        if ui.button("VDecimate").clicked() {
                            self.launch_vs_op(VsOp::Vdecimate, entry.path.clone(), cfg, status);
                        }
                        if ui.button("Fix A/V Sync").clicked() {
                            self.launch_fix_sync(entry.path.clone(), cfg, status);
                        }
                        if ui.button("Viewer Encode").clicked() {
                            self.launch_pipeline(
                                format!("Viewer Encode {}", entry.name),
                                cfg.viewer_encode_script(),
                                entry.path.clone(),
                                &[],
                                &[],
                                cfg,
                                status,
                            );
                        }
                    }
                    FileKind::EditMasterVD => {
                        if ui.button("Viewer Encode").clicked() {
                            self.launch_pipeline(
                                format!("Viewer Encode {}", entry.name),
                                cfg.viewer_encode_script(),
                                entry.path.clone(),
                                &[],
                                &[],
                                cfg,
                                status,
                            );
                        }
                        if ui.button("Upscale Film").clicked() {
                            let out = Self::upscale_output(&entry.path, cfg);
                            self.launch_upscale(
                                format!("Upscale Film {}", entry.name),
                                cfg.upscale_script(),
                                entry.path.clone(),
                                out,
                                cfg,
                                status,
                            );
                        }
                        if ui.button("Upscale Film B&W").clicked() {
                            let out = Self::upscale_output(&entry.path, cfg);
                            self.launch_upscale(
                                format!("Upscale Film B&W {}", entry.name),
                                cfg.upscale_bw_script(),
                                entry.path.clone(),
                                out,
                                cfg,
                                status,
                            );
                        }
                        if ui.button("Upscale Anime").clicked() {
                            let out = Self::upscale_output(&entry.path, cfg);
                            self.launch_upscale(
                                format!("Upscale Anime {}", entry.name),
                                cfg.upscale_anime_script(),
                                entry.path.clone(),
                                out,
                                cfg,
                                status,
                            );
                        }
                    }
                    FileKind::Viewer => {
                        if ui.button("Upscale").clicked() {
                            let out = Self::upscale_output(&entry.path, cfg);
                            self.launch_upscale(
                                format!("Upscale {}", entry.name),
                                cfg.upscale_script(),
                                entry.path.clone(),
                                out,
                                cfg,
                                status,
                            );
                        }
                        if ui.button("Upscale B&W").clicked() {
                            let out = Self::upscale_output(&entry.path, cfg);
                            self.launch_upscale(
                                format!("Upscale B&W {}", entry.name),
                                cfg.upscale_bw_script(),
                                entry.path.clone(),
                                out,
                                cfg,
                                status,
                            );
                        }
                        if ui.button("Upscale Anime").clicked() {
                            let out = Self::upscale_output(&entry.path, cfg);
                            self.launch_upscale(
                                format!("Upscale Anime {}", entry.name),
                                cfg.upscale_anime_script(),
                                entry.path.clone(),
                                out,
                                cfg,
                                status,
                            );
                        }
                    }
                }

                if matches!(entry.kind, FileKind::Viewer) && ui.button("Rename…").clicked() {
                    let suggestion = suggest_viewer_name(&entry.path);
                    self.rename_state = Some((entry.path.clone(), suggestion));
                }
                if ui.button("🗑 Delete").clicked() {
                    self.confirm_delete = Some(entry.path.clone());
                }
            });
        });

        // Delete confirmation
        if let Some(ref path) = self.confirm_delete.clone()
            && path == &entry.path
        {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Move to Trash?").color(egui::Color32::RED));
                if ui.button("✓ Yes").clicked() {
                    if let Err(e) = trash::delete(path) {
                        *status = format!("Trash failed: {e}");
                    } else {
                        *status = format!("Trashed {}", entry.name);
                    }
                    self.confirm_delete = None;
                    self.library.refresh(cfg);
                }
                if ui.button("✗ No").clicked() {
                    self.confirm_delete = None;
                }
            });
        }

        // Rename UI (two-pass borrow pattern)
        let rename_action: Option<Result<String, ()>> =
            if let Some((ref orig, ref mut edit)) = self.rename_state {
                if orig == &entry.path {
                    ui.separator();
                    ui.label(egui::RichText::new("Rename to:").small().weak());
                    ui.add(
                        egui::TextEdit::singleline(edit)
                            .desired_width(f32::INFINITY)
                            .hint_text("filename.mkv"),
                    );
                    let mut action = None;
                    ui.horizontal(|ui| {
                        if ui.button("✓ OK").clicked() {
                            action = Some(Ok(edit.clone()));
                        }
                        if ui.button("✗ Cancel").clicked() {
                            action = Some(Err(()));
                        }
                    });
                    action
                } else {
                    None
                }
            } else {
                None
            };

        match rename_action {
            Some(Ok(new_name)) => {
                self.rename_state = None;
                let new_name = new_name.trim().to_owned();
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
                                // Keep the renamed file selected so the action
                                // panel (and Rename button) doesn't disappear —
                                // otherwise a second rename requires re-clicking
                                // the row first.
                                self.library.selected =
                                    self.library.entries.iter().position(|e| e.path == new_path);
                            }
                            Err(e) => *status = format!("Rename failed: {e}"),
                        }
                    }
                }
            }
            Some(Err(())) => {
                self.rename_state = None;
            }
            None => {}
        }

        // Running job progress
        let (do_toggle_pause, do_stop_after_seg, do_cancel) = if let Some(ref job) = self.pipeline {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("● {}", job.label))
                    .color(egui::Color32::from_rgb(80, 200, 80))
                    .small(),
            );

            let pulse = {
                let t = ctx.input(|i| i.time);
                ((t * 0.4).sin() * 0.5 + 0.5) as f32
            };

            if job.is_upscale {
                let total_fill = job.total_progress().unwrap_or(0.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Total  {}/{} segments",
                        job.completed_segments, job.total_segments
                    ))
                    .small(),
                );
                ui.add(egui::ProgressBar::new(total_fill).animate(false));

                let seg_fill = job.segment_progress().unwrap_or(pulse);
                ui.label(egui::RichText::new("Segment").small());
                ui.add(egui::ProgressBar::new(seg_fill).animate(true));
            } else {
                let fill = job.progress().unwrap_or(pulse);
                ui.add(egui::ProgressBar::new(fill).animate(true));
            }

            let frame_txt = if job.is_upscale {
                format!(
                    "frame {} / {}  {}",
                    job.upscaled_frames,
                    job.segment_frames,
                    job.elapsed_str()
                )
            } else if job.total_frames > 0 {
                format!(
                    "frame {} / {}  {}",
                    job.current_frame,
                    job.total_frames,
                    job.elapsed_str()
                )
            } else {
                format!("frame {}  {}", job.current_frame, job.elapsed_str())
            };
            ui.label(egui::RichText::new(frame_txt).small());

            let mut toggle_pause = false;
            let mut stop_after = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                if job.is_upscale {
                    let pause_label = if job.paused { "Resume" } else { "Pause" };
                    if ui.button(pause_label).clicked() {
                        toggle_pause = true;
                    }
                    if job.stopping_after_segment() {
                        ui.label(egui::RichText::new("Stopping…").weak().small());
                    } else if ui.button("Stop after Segment").clicked() {
                        stop_after = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });

            ctx.request_repaint_after(std::time::Duration::from_secs(1));
            (toggle_pause, stop_after, cancel)
        } else {
            (false, false, false)
        };

        if do_toggle_pause && let Some(ref mut job) = self.pipeline {
            job.toggle_pause();
        }
        if do_stop_after_seg && let Some(ref mut job) = self.pipeline {
            job.request_stop_after_segment();
        }
        if do_cancel && let Some(ref job) = self.pipeline {
            job.cancel();
        }
    }
}

// -----------------------------------------------------------------------
// Free helpers
// -----------------------------------------------------------------------

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

fn load_jpeg_as_egui_image(path: &std::path::Path) -> Option<egui::ColorImage> {
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
