use std::path::PathBuf;

use crate::persist::PersistedPaths;

pub struct Config {
    pub videos_dir: PathBuf,
    /// Scratch root for chunked/resumable upscale checkpoints. GUI-editable;
    /// see `from_paths`.
    pub work_root: PathBuf,
    pub v4l2_device: String,
    pub max_capture_duration: String,
}

impl Config {
    fn home() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home/user".into()))
    }

    /// Env fallback (matches the bash scripts' own WORK_ROOT convention),
    /// then the hardcoded generic default, only used when neither a GUI
    /// override nor WORK_ROOT is set. The hardcoded default lives under
    /// videos_dir rather than assuming an external "scratch" mount exists —
    /// a machine-specific assumption that isn't true for a fresh clone and
    /// fails with a permission error rather than a missing-file one, which
    /// is a much less obvious thing to debug. Users with a real external
    /// scratch drive should set WORK_ROOT or use the GUI's Working
    /// Directories window.
    pub fn default_work_root() -> PathBuf {
        std::env::var("WORK_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Self::home().join("Videos/vhs_upscale_work"))
    }

    /// Layers persisted GUI overrides over env vars over hardcoded defaults.
    /// A blank/absent override falls through to the next tier.
    pub fn from_paths(paths: &PersistedPaths) -> Self {
        let work_root = paths
            .work_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_work_root);
        Self {
            videos_dir: Self::home().join("Videos"),
            work_root,
            v4l2_device: "/dev/v4l/by-id/usb-MACROSIL_AV_TO_USB2.0-video-index0".into(),
            max_capture_duration: "02:00:00".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::from_paths(&PersistedPaths::default())
    }
}

impl Config {
    pub fn archival_dir(&self) -> PathBuf {
        self.videos_dir.join("captures/archival")
    }
    pub fn stabilized_dir(&self) -> PathBuf {
        self.videos_dir.join("captures/stabilized")
    }
    pub fn viewer_dir(&self) -> PathBuf {
        self.videos_dir.join("captures/viewer")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.videos_dir.join("logs")
    }
    pub fn upscale_work_root(&self) -> PathBuf {
        self.work_root.clone()
    }
}

/// Where the sibling vhs-cli checkout lives, for the ignored tests that compare the native code
/// with the real scripts. Nothing outside the tests uses it.
#[cfg(test)]
pub fn vhs_cli_dir() -> PathBuf {
    Config::home().join("Videos/vhs-cli")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_paths_prefers_gui_override() {
        let paths = PersistedPaths {
            work_root: Some("/tmp/custom-work-root".into()),
        };
        let cfg = Config::from_paths(&paths);
        assert_eq!(cfg.work_root, PathBuf::from("/tmp/custom-work-root"));
    }

    #[test]
    fn from_paths_falls_back_when_blank_or_absent() {
        let blank = PersistedPaths {
            work_root: Some("   ".into()),
        };
        let absent = PersistedPaths::default();

        let cfg_blank = Config::from_paths(&blank);
        let cfg_absent = Config::from_paths(&absent);

        assert_eq!(cfg_blank.work_root, Config::default_work_root());
        assert_eq!(cfg_absent.work_root, Config::default_work_root());
    }
}
