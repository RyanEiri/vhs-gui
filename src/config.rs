use std::path::PathBuf;

use crate::persist::PersistedPaths;

pub struct Config {
    pub videos_dir: PathBuf,
    /// `~/Videos/vhs-cli` — the sibling vhs-cli repo. Only referenced for
    /// operations not yet natively ported (see the `*_script()` accessors
    /// below, each marked with the phase that will remove it). GUI-editable
    /// via the Working Directories window; see `from_paths`.
    pub scripts_dir: PathBuf,
    /// Scratch root for chunked/resumable upscale checkpoints. GUI-editable;
    /// see `from_paths`. Also passed explicitly as `WORK_ROOT` when spawning
    /// the upscale scripts (see `panels::upscale::launch_upscale`) so the
    /// spawned process can never resolve a different directory than the one
    /// Rust is tracking checkpoints against.
    pub work_root: PathBuf,
    pub v4l2_device: String,
    pub max_capture_duration: String,
}

impl Config {
    fn home() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home/user".into()))
    }

    /// Hardcoded fallback, only used when neither a GUI override nor
    /// SCRIPTS_DIR is set.
    pub fn default_scripts_dir() -> PathBuf {
        Self::home().join("Videos/vhs-cli")
    }

    /// Env fallback (matches the bash scripts' own WORK_ROOT convention),
    /// then the hardcoded generic default, only used when neither a GUI
    /// override nor WORK_ROOT is set.
    pub fn default_work_root() -> PathBuf {
        std::env::var("WORK_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
                PathBuf::from(format!("/media/{user}/scratch/Videos/vhs_upscale_work"))
            })
    }

    /// Layers persisted GUI overrides over env vars over hardcoded defaults.
    /// A blank/absent override falls through to the next tier.
    pub fn from_paths(paths: &PersistedPaths) -> Self {
        let scripts_dir = paths
            .scripts_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_scripts_dir);
        let work_root = paths
            .work_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_work_root);
        Self {
            videos_dir: Self::home().join("Videos"),
            scripts_dir,
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
    pub fn capture_pgid_file(&self) -> PathBuf {
        self.log_dir().join("capture.pgid")
    }
    pub fn capture_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_capture_ffmpeg.sh")
    }
    /// `vhs-env/tools/` under the sibling vhs-cli repo — where the
    /// VapourSynth `.vpy` scripts live. Used by the native `vs_ops` catalog.
    pub fn tools_dir(&self) -> PathBuf {
        self.scripts_dir.join("vhs-env/tools")
    }
    // TODO(native-port): remove once Denoise is natively ported.
    pub fn denoise_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_denoise.sh")
    }
    // TODO(native-port): remove once Denoise+QTGMC is natively ported.
    pub fn process_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_process.sh")
    }
    // TODO(native-port): remove once Viewer Encode is natively ported.
    pub fn viewer_encode_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_viewer_encode.sh")
    }
    // TODO(native-port): remove once Upscale is natively ported.
    pub fn upscale_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_upscale.sh")
    }
    // TODO(native-port): remove once Upscale Anime is natively ported.
    pub fn upscale_anime_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_upscale_anime.sh")
    }
    // TODO(native-port): remove once Upscale B&W is natively ported.
    pub fn upscale_bw_script(&self) -> PathBuf {
        self.scripts_dir.join("vhs_upscale_bw.sh")
    }
    pub fn upscale_work_root(&self) -> PathBuf {
        self.work_root.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_paths_prefers_gui_override() {
        let paths = PersistedPaths {
            scripts_dir: Some("/tmp/custom-scripts".into()),
            work_root: Some("/tmp/custom-work-root".into()),
        };
        let cfg = Config::from_paths(&paths);
        assert_eq!(cfg.scripts_dir, PathBuf::from("/tmp/custom-scripts"));
        assert_eq!(cfg.work_root, PathBuf::from("/tmp/custom-work-root"));
    }

    #[test]
    fn from_paths_falls_back_when_blank_or_absent() {
        let blank = PersistedPaths {
            scripts_dir: Some("   ".into()),
            work_root: Some(String::new()),
        };
        let absent = PersistedPaths::default();

        let cfg_blank = Config::from_paths(&blank);
        let cfg_absent = Config::from_paths(&absent);

        assert_eq!(cfg_blank.scripts_dir, Config::default_scripts_dir());
        assert_eq!(cfg_blank.work_root, Config::default_work_root());
        assert_eq!(cfg_absent.scripts_dir, Config::default_scripts_dir());
        assert_eq!(cfg_absent.work_root, Config::default_work_root());
    }
}
