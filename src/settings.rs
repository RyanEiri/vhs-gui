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

impl Backend {
    pub fn env_value(&self) -> &str {
        match self {
            Self::Rocm => "rocm",
            Self::Vulkan => "vulkan",
        }
    }
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
    pub denoise: bool,
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
            denoise: false,
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
        // Auto-infer internal scale from model name.
        if let Some(name) = self.selected_model()
            && let Some(s) = infer_scale(name)
        {
            self.internal_scale = s;
        }
    }

    /// Build the `(envs, positional_args)` pair for passing to `PipelineJob::start`.
    ///
    /// `extra_args` layout: `[output_path, segment_secs, crf]`
    /// Caller must keep the owned strings alive for the duration of `start()`.
    pub fn to_launch(&self, output_path: &std::path::Path) -> (Vec<(String, String)>, Vec<String>) {
        let mut envs: Vec<(String, String)> = vec![
            ("CRUSH".into(), self.crush.env_value().into()),
            (
                "BRIGHTNESS".into(),
                self.brightness.env_value(&self.brightness_custom),
            ),
            ("UPSCALE_BACKEND".into(), self.backend.env_value().into()),
            ("INTERNAL_SCALE".into(), self.internal_scale.to_string()),
            ("FINAL_SCALE".into(), self.final_scale.to_string()),
        ];

        match self.backend {
            Backend::Rocm => {
                // ROCm: BATCH_SIZE controls frames-per-call in driver.py.
                envs.push(("BATCH_SIZE".into(), self.batch_size.to_string()));
                // MODELS_DIR is ignored by ROCm backend; omit to avoid confusion.
                if let Some(m) = self.selected_model() {
                    envs.push(("MODEL".into(), m.into()));
                }
            }
            Backend::Vulkan => {
                envs.push((
                    "MODELS_DIR".into(),
                    self.effective_models_dir().to_string_lossy().into_owned(),
                ));
                if let Some(m) = self.selected_model() {
                    envs.push(("MODEL".into(), m.into()));
                }
            }
        }

        if self.denoise {
            envs.push(("DENOISE".into(), "1".into()));
        }

        let args = vec![
            output_path.to_string_lossy().into_owned(),
            self.segment_secs.to_string(),
            self.crf.to_string(),
        ];

        (envs, args)
    }
}
