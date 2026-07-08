use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::panels::ViewMode;
use crate::settings::{
    Backend, BrightnessPreset, CrushPreset, UpscaleSettings, preset_models_dirs,
};
use crate::v4l2::V4l2Controls;

// -----------------------------------------------------------------------
// PersistedUpscaleSettings — serde-friendly subset of UpscaleSettings
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PersistedUpscaleSettings {
    pub crush: CrushPreset,
    pub brightness: BrightnessPreset,
    pub brightness_custom: String,
    pub models_dir_idx: usize,
    pub models_dir_custom: String,
    /// Model stored by name so it survives directory rescans.
    pub model_name: Option<String>,
    pub internal_scale: u8,
    pub final_scale: u8,
    pub crf: u32,
    pub segment_secs: u32,
    pub backend: Backend,
    pub batch_size: u32,
    pub denoise: bool,
}

impl Default for PersistedUpscaleSettings {
    fn default() -> Self {
        let presets = preset_models_dirs();
        let dir = presets.first().map(|(_, p)| p.clone()).unwrap_or_default();
        Self {
            crush: CrushPreset::None,
            brightness: BrightnessPreset::None,
            brightness_custom: String::new(),
            models_dir_idx: 0,
            models_dir_custom: dir.to_string_lossy().into_owned(),
            model_name: None,
            internal_scale: 4,
            final_scale: 2,
            crf: 21,
            segment_secs: 30,
            backend: Backend::Rocm,
            batch_size: 2,
            denoise: false,
        }
    }
}

// -----------------------------------------------------------------------
// AppSettings — top-level TOML document
// -----------------------------------------------------------------------

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub mode: ViewMode,
    pub v4l2: BTreeMap<String, i32>,
    pub upscale: PersistedUpscaleSettings,
}

impl AppSettings {
    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config/vhs-gui/config.toml")
    }

    /// Load from disk; returns Default on any error so startup is never blocked.
    pub fn load() -> Self {
        let path = Self::config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        toml::from_str(&text).unwrap_or_default()
    }

    /// Write current state to `~/.config/vhs-gui/config.toml`.
    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }

    /// Apply loaded settings to the live app structs.
    /// Fires v4l2-ctl for each hardware control immediately so the
    /// capture card reflects the persisted preset on startup.
    pub fn apply_to(
        &self,
        view_mode: &mut ViewMode,
        v4l2: &mut V4l2Controls,
        upscale: &mut UpscaleSettings,
    ) {
        *view_mode = self.mode;

        if !self.v4l2.is_empty() {
            v4l2.apply_values(&self.v4l2);
        }

        upscale.crush = self.upscale.crush.clone();
        upscale.brightness = self.upscale.brightness.clone();
        upscale.brightness_custom = self.upscale.brightness_custom.clone();
        upscale.models_dir_idx = self.upscale.models_dir_idx;
        upscale.models_dir_custom = self.upscale.models_dir_custom.clone();
        upscale.internal_scale = self.upscale.internal_scale;
        upscale.final_scale = self.upscale.final_scale;
        upscale.crf = self.upscale.crf;
        upscale.segment_secs = self.upscale.segment_secs;
        upscale.backend = self.upscale.backend.clone();
        upscale.batch_size = self.upscale.batch_size;
        upscale.denoise = self.upscale.denoise;

        // Rescan models for the restored directory, then find the saved model by name.
        upscale.rescan();
        if let Some(ref name) = self.upscale.model_name {
            let list = upscale.effective_model_list();
            if let Some(idx) = list.iter().position(|m| *m == name.as_str()) {
                upscale.model_idx = idx;
            }
        }
    }

    /// Snapshot current live state into an AppSettings ready for saving.
    pub fn capture_from(
        view_mode: &ViewMode,
        v4l2: &V4l2Controls,
        upscale: &UpscaleSettings,
    ) -> Self {
        let model_name = upscale.selected_model().map(|s| s.to_owned());
        Self {
            mode: *view_mode,
            v4l2: v4l2.to_preset(),
            upscale: PersistedUpscaleSettings {
                crush: upscale.crush.clone(),
                brightness: upscale.brightness.clone(),
                brightness_custom: upscale.brightness_custom.clone(),
                models_dir_idx: upscale.models_dir_idx,
                models_dir_custom: upscale.models_dir_custom.clone(),
                model_name,
                internal_scale: upscale.internal_scale,
                final_scale: upscale.final_scale,
                crf: upscale.crf,
                segment_secs: upscale.segment_secs,
                backend: upscale.backend.clone(),
                batch_size: upscale.batch_size,
                denoise: upscale.denoise,
            },
        }
    }
}
