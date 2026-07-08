use std::path::PathBuf;

pub struct Config {
    pub videos_dir: PathBuf,
    /// `~/Videos/vhs-cli` — the sibling vhs-cli repo. Only referenced for
    /// operations not yet natively ported (see the `*_script()` accessors
    /// below, each marked with the phase that will remove it).
    pub scripts_dir: PathBuf,
    pub v4l2_device: String,
    pub max_capture_duration: String,
    pub capture_script: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home/ryan".into()));
        let videos = home.join("Videos");
        let scripts = videos.join("vhs-cli");
        Self {
            capture_script: scripts.join("vhs_capture_ffmpeg.sh"),
            v4l2_device: "/dev/v4l/by-id/usb-MACROSIL_AV_TO_USB2.0-video-index0".into(),
            max_capture_duration: "02:00:00".into(),
            videos_dir: videos,
            scripts_dir: scripts,
        }
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
        PathBuf::from("/media/ryan/Patriot/Videos/vhs_upscale_work")
    }
}
