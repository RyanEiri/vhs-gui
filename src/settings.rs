use std::path::PathBuf;

// -----------------------------------------------------------------------
// Enums
// -----------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrushPreset {
    #[default]
    None,
    Small,
    Medium,
    Heavy,
}

impl CrushPreset {
    pub const ALL: &'static [Self] = &[Self::None, Self::Small, Self::Medium, Self::Heavy];
    pub fn label(&self) -> &str {
        match self {
            Self::None => "None",
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Heavy => "Heavy",
        }
    }
    pub fn env_value(&self) -> &str {
        match self {
            Self::None => "none",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Heavy => "heavy",
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrightnessPreset {
    #[default]
    None,
    Low,
    Medium,
    High,
    Custom,
}

impl BrightnessPreset {
    pub const ALL: &'static [Self] = &[
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Custom,
    ];
    pub fn label(&self) -> &str {
        match self {
            Self::None => "None (0)",
            Self::Low => "Low (0.02)",
            Self::Medium => "Medium (0.05)",
            Self::High => "High (0.095)",
            Self::Custom => "Custom…",
        }
    }
    pub fn env_value(&self, custom: &str) -> String {
        match self {
            Self::None => "none".into(),
            Self::Low => "low".into(),
            Self::Medium => "medium".into(),
            Self::High => "high".into(),
            Self::Custom => custom.trim().to_owned(),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Rocm,
    Vulkan,
}

// -----------------------------------------------------------------------
// Model discovery
// -----------------------------------------------------------------------

/// Model names available on the ROCm backend (driver.py MODEL_MAP).
pub const ROCM_MODELS: &[&str] = &[
    "realesrgan-x4plus",
    "realesrgan-x4plus-anime",
    "realesrgan-x2plus",
    "2x_VHS-Film",
    "ToonVHS-1x",
    "VHS-Sharpen-1x",
];

/// Preset models directories: (display label, path with ~ expanded).
pub fn preset_models_dirs() -> Vec<(String, PathBuf)> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    vec![
        ("Standard".into(), home.join("opt/realesrgan-ncnn/models")),
        ("Downloads/ncnn".into(), home.join("Downloads/models/ncnn")),
    ]
}

/// `realesrgan-ncnn-vulkan` hardcodes its network architecture selection by
/// matching the `-n` model name against these families (see its `-n` help
/// text). Any other name — even with valid `.param`/`.bin` files present —
/// segfaults the binary instead of erroring gracefully. Verified empirically:
/// `2x_VHS-Film`, `ToonVHS-1x`, and `VHS-Sharpen-1x` all crash it (exit 139)
/// despite having ncnn pairs on disk; only these families actually run.
fn is_vulkan_compatible(name: &str) -> bool {
    matches!(
        name,
        "realesrgan-x4plus" | "realesrgan-x4plus-anime" | "realesrnet-x4plus"
    ) || name.starts_with("realesr-animevideov3")
}

/// Scan a directory for `*.param` + `*.bin` pairs and return sorted base names,
/// filtered to names `realesrgan-ncnn-vulkan` actually supports.
pub fn scan_models(dir: &PathBuf) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? == "param" {
                let stem = p.file_stem()?.to_str()?.to_owned();
                if p.with_extension("bin").exists() && is_vulkan_compatible(&stem) {
                    return Some(stem);
                }
            }
            None
        })
        .collect();
    names.sort();
    names
}

/// Infer the native scale from a model name.  Returns `Some` when certain.
/// Handles prefix patterns (2x_, 4x_), -xN suffixes (realesr-animevideov3-x4),
/// and -Nx suffixes (ToonVHS-1x, VHS-Sharpen-1x).
pub fn infer_scale(model_name: &str) -> Option<u8> {
    let n = model_name.to_lowercase();
    for scale in [1u8, 2, 3, 4] {
        let s = scale.to_string();
        if n.starts_with(&format!("{s}x_"))
            || n.contains(&format!("-x{s}"))
            || n.contains(&format!("-{s}x"))
        {
            return Some(scale);
        }
    }
    None
}

// -----------------------------------------------------------------------
// UpscaleSettings
// -----------------------------------------------------------------------

pub struct UpscaleSettings {
    pub crush: CrushPreset,
    pub brightness: BrightnessPreset,
    pub brightness_custom: String,
    /// Index into `preset_models_dirs()`. If == presets.len(), use custom dir.
    pub models_dir_idx: usize,
    pub models_dir_custom: String,
    /// Model names available given the current backend + models dir.
    pub scanned_models: Vec<String>,
    /// Selected index into the effective model list (ROCm or scanned).
    pub model_idx: usize,
    pub internal_scale: u8,
    pub final_scale: u8,
    pub crf: u32,
    pub segment_secs: u32,
    pub backend: Backend,
    pub batch_size: u32,
}

impl Default for UpscaleSettings {
    fn default() -> Self {
        let presets = preset_models_dirs();
        let dir = presets.first().map(|(_, p)| p.clone()).unwrap_or_default();
        let scanned = scan_models(&dir);
        let custom_str = dir.to_string_lossy().into_owned();
        Self {
            crush: CrushPreset::None,
            brightness: BrightnessPreset::None,
            brightness_custom: String::new(),
            models_dir_idx: 0,
            models_dir_custom: custom_str,
            scanned_models: scanned,
            model_idx: 0,
            internal_scale: 4,
            final_scale: 2,
            crf: 21,
            segment_secs: 30,
            backend: Backend::Rocm,
            batch_size: 2,
        }
    }
}

impl UpscaleSettings {
    /// Resolved models directory path.
    pub fn effective_models_dir(&self) -> PathBuf {
        let presets = preset_models_dirs();
        if self.models_dir_idx < presets.len() {
            presets[self.models_dir_idx].1.clone()
        } else {
            PathBuf::from(&self.models_dir_custom)
        }
    }

    /// The model name list valid for the current backend.
    pub fn effective_model_list(&self) -> Vec<&str> {
        match self.backend {
            Backend::Rocm => ROCM_MODELS.to_vec(),
            Backend::Vulkan => self.scanned_models.iter().map(|s| s.as_str()).collect(),
        }
    }

    /// Selected model name, or None if the list is empty / index is out of range.
    pub fn selected_model(&self) -> Option<&str> {
        self.effective_model_list().get(self.model_idx).copied()
    }

    /// Re-scan the models directory and clamp the model index.
    pub fn rescan(&mut self) {
        self.scanned_models = scan_models(&self.effective_models_dir());
        let max = self.effective_model_list().len().saturating_sub(1);
        if self.model_idx > max {
            self.model_idx = 0;
        }
        self.sync_scale_to_model();
    }

    /// Force internal_scale to match the selected model's inferred native
    /// scale, if one is known (models with no inferrable scale keep whatever
    /// the user picked). Previously this correction only ran inside the
    /// settings panel's per-frame UI code, so a stale internal_scale from an
    /// earlier model selection could silently survive if that panel was
    /// closed — reaching a launch and corrupting a resume's config
    /// fingerprint match against an existing checkpoint. Called here, from
    /// rescan(), and unconditionally at the top of launch_upscale() so a
    /// launched job can never disagree with its own model's scale
    /// regardless of UI panel state.
    pub fn sync_scale_to_model(&mut self) {
        if let Some(s) = self.selected_model().and_then(infer_scale) {
            self.internal_scale = s;
        }
        self.clamp_final_scale();
    }

    /// Keep `final_scale` something the upscale scripts accept: they downscale
    /// from the internal scale, so the final scale can't exceed it and must
    /// divide it evenly (otherwise they exit with "INTERNAL_SCALE must be
    /// evenly divisible by FINAL_SCALE"). A fixed 1x model such as
    /// VHS-Sharpen-1x therefore forces a 1x final scale. Picks the largest
    /// valid option, so a valid choice is never lowered.
    pub fn clamp_final_scale(&mut self) {
        let valid = |f: u8| f <= self.internal_scale && self.internal_scale.is_multiple_of(f);
        if !valid(self.final_scale) {
            self.final_scale = [4u8, 2, 1].into_iter().find(|&f| valid(f)).unwrap_or(1);
        }
    }

    /// The settings as a job for the native engine.
    pub fn to_job_spec(
        &self,
        variant: crate::upscale::plan::Variant,
        input: &std::path::Path,
        output: &std::path::Path,
        work_root: &std::path::Path,
    ) -> crate::upscale::plan::JobSpec {
        let mut spec = crate::upscale::plan::JobSpec::defaults(variant, input, output, work_root);
        spec.segment_secs = self.segment_secs;
        spec.crf = self.crf;
        spec.backend = self.backend.clone();
        if let Some(m) = self.selected_model() {
            spec.model = m.into();
        }
        // The scripts only get MODELS_DIR for Vulkan and otherwise use their own default, which
        // `JobSpec::defaults` already holds; keeping that makes the resume fingerprint match.
        if self.backend == Backend::Vulkan {
            spec.models_dir = self.effective_models_dir();
        }
        spec.internal_scale = u32::from(self.internal_scale);
        spec.final_scale = u32::from(self.final_scale);
        spec.crush = self.crush.env_value().into();
        spec.brightness = self.brightness.env_value(&self.brightness_custom);
        if self.backend == Backend::Rocm {
            spec.batch_size = Some(self.batch_size);
        }
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upscale::plan::Variant;
    use std::path::Path;

    /// Every knob in the panel has to reach the job spec the engine runs.
    #[test]
    fn the_job_spec_carries_the_settings() {
        for backend in [Backend::Rocm, Backend::Vulkan] {
            let s = UpscaleSettings {
                backend: backend.clone(),
                crush: CrushPreset::Medium,
                brightness: BrightnessPreset::Custom,
                brightness_custom: " 0.03 ".into(),
                crf: 18,
                segment_secs: 20,
                batch_size: 3,
                ..Default::default()
            };
            let spec = s.to_job_spec(
                Variant::Standard,
                Path::new("/in/i.mkv"),
                Path::new("/out/o.mkv"),
                Path::new("/work"),
            );

            assert_eq!(spec.crush, "medium");
            assert_eq!(spec.brightness, "0.03"); // trimmed
            assert_eq!(spec.internal_scale, u32::from(s.internal_scale));
            assert_eq!(spec.final_scale, u32::from(s.final_scale));
            assert_eq!((spec.segment_secs, spec.crf), (20, 18));
            assert_eq!(spec.crop, None);
            match backend {
                Backend::Rocm => {
                    assert_eq!(spec.backend_name(), "rocm");
                    assert_eq!(spec.batch_size, Some(3));
                    // The ROCm driver ignores the models directory, so the spec keeps the default
                    // (it is part of the resume fingerprint).
                    let default = crate::upscale::plan::JobSpec::defaults(
                        Variant::Standard,
                        Path::new("/in/i.mkv"),
                        Path::new("/out/o.mkv"),
                        Path::new("/work"),
                    );
                    assert_eq!(spec.models_dir, default.models_dir);
                }
                Backend::Vulkan => {
                    assert_eq!(spec.backend_name(), "vulkan");
                    assert_eq!(spec.batch_size, None);
                    assert_eq!(spec.models_dir, s.effective_models_dir());
                }
            }
        }
    }

    #[test]
    fn sync_scale_to_model_corrects_stale_scale_from_prior_model() {
        // Reproduces the real bug: internal_scale left at 4 (e.g. from a
        // previously selected 4x model, with the settings panel closed so
        // the UI-only correction never ran), then the user selects a 1x
        // model. Before this fix, a launch in this state would silently use
        // the wrong scale and corrupt any resume fingerprint match.
        let mut s = UpscaleSettings {
            backend: Backend::Rocm,
            ..Default::default()
        };
        let idx = s
            .effective_model_list()
            .iter()
            .position(|m| *m == "VHS-Sharpen-1x")
            .unwrap();
        s.model_idx = idx;
        s.internal_scale = 4;
        s.sync_scale_to_model();
        assert_eq!(s.internal_scale, 1);
    }

    #[test]
    fn clamp_final_scale_forces_1x_for_1x_model() {
        let mut s = UpscaleSettings {
            backend: Backend::Rocm,
            final_scale: 2,
            ..Default::default()
        };
        let idx = s
            .effective_model_list()
            .iter()
            .position(|m| *m == "VHS-Sharpen-1x")
            .unwrap();
        s.model_idx = idx;
        s.sync_scale_to_model();
        assert_eq!((s.internal_scale, s.final_scale), (1, 1));
    }

    #[test]
    fn clamp_final_scale_keeps_valid_choices() {
        for (internal, final_scale) in [(4u8, 4u8), (4, 2), (4, 1), (2, 2), (2, 1), (1, 1)] {
            let mut s = UpscaleSettings {
                internal_scale: internal,
                final_scale,
                ..Default::default()
            };
            s.clamp_final_scale();
            assert_eq!(s.final_scale, final_scale, "internal {internal}");
        }
    }

    #[test]
    fn clamp_final_scale_picks_largest_valid_option() {
        for (internal, final_scale, want) in [(2u8, 4u8, 2u8), (3, 2, 1), (3, 4, 1), (1, 4, 1)] {
            let mut s = UpscaleSettings {
                internal_scale: internal,
                final_scale,
                ..Default::default()
            };
            s.clamp_final_scale();
            assert_eq!(
                s.final_scale, want,
                "internal {internal} final {final_scale}"
            );
        }
    }

    #[test]
    fn sync_scale_to_model_leaves_unrecognized_model_alone() {
        let mut s = UpscaleSettings {
            backend: Backend::Vulkan,
            scanned_models: vec!["some-community-model".into()],
            model_idx: 0,
            internal_scale: 3,
            ..Default::default()
        };
        s.sync_scale_to_model();
        assert_eq!(s.internal_scale, 3);
    }

    #[test]
    fn rescan_also_corrects_stale_scale() {
        let mut s = UpscaleSettings {
            backend: Backend::Rocm,
            ..Default::default()
        };
        let idx = s
            .effective_model_list()
            .iter()
            .position(|m| *m == "realesrgan-x2plus")
            .unwrap();
        s.model_idx = idx;
        s.internal_scale = 4;
        s.rescan();
        assert_eq!(s.internal_scale, 2);
    }
}
