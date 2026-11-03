//! Pure planning for an upscale job. No I/O and no processes, so the golden tests can compare
//! every value and argument list with what the bash scripts produce.
//!
//! Wherever the scripts have a quirk, this reproduces it (for example a brightness of `0.0` is not
//! the string `0`, so it adds an `eq` filter), because the resume fingerprint and the output must
//! stay identical to what a script would have made.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::settings::Backend;

/// Which of the three scripts a job stands in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Standard,
    Anime,
    Bw,
}

impl Variant {
    /// `SCRIPT=` in the fingerprint: the script this job replaces, so a work directory started by
    /// a script can be resumed natively and the other way round.
    pub fn script_name(self) -> &'static str {
        match self {
            Variant::Standard => "vhs_upscale.sh",
            Variant::Anime => "vhs_upscale_anime.sh",
            Variant::Bw => "vhs_upscale_bw.sh",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Variant::Anime => "realesrgan-x4plus-anime",
            _ => "realesrgan-x4plus",
        }
    }

    /// Largest allowed drift between the source and the summed segment durations, in percent.
    pub fn max_drift_pct(self) -> f64 {
        match self {
            Variant::Anime => 8.0,
            _ => 2.0,
        }
    }

    /// The fingerprint key and log label for the pre-filter (`BW_FILTER` in the B&W script).
    pub fn filter_key(self) -> &'static str {
        match self {
            Variant::Bw => "BW_FILTER",
            _ => "PRE_VF",
        }
    }
}

/// Everything a job needs, written to `job.toml` by the GUI and read by the worker. Defaults match
/// the scripts'.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub variant: Variant,
    pub input: PathBuf,
    pub output: PathBuf,
    pub work_root: PathBuf,
    pub segment_secs: u32,
    pub crf: u32,
    pub preset: String,
    pub backend: Backend,
    pub model: String,
    pub models_dir: PathBuf,
    pub internal_scale: u32,
    pub final_scale: u32,
    /// `None`: the backend's default (400 for Vulkan, 0 for ROCm).
    pub tile_size: Option<u32>,
    pub threads: String,
    pub vk_device: u32,
    pub jpeg_quality: u32,
    /// `none`, `small`, `medium` or `heavy`.
    pub crush: String,
    /// A named level, a raw float, or empty for the crush preset's own default.
    pub brightness: String,
    pub target_dar: String,
    pub allow_mixed: bool,
    /// ROCm only: frames per call, passed to the driver as `BATCH_SIZE`.
    pub batch_size: Option<u32>,
    /// Skip the final A/V sync correction (tests only).
    #[serde(default)]
    pub skip_sync_fix: bool,
}

impl JobSpec {
    /// The scripts' defaults for `variant`.
    pub fn defaults(variant: Variant, input: &Path, output: &Path, work_root: &Path) -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        Self {
            variant,
            input: input.into(),
            output: output.into(),
            work_root: work_root.into(),
            segment_secs: 30,
            crf: 21,
            preset: "veryfast".into(),
            backend: Backend::Vulkan,
            model: variant.default_model().into(),
            models_dir: PathBuf::from(home).join("opt/realesrgan-ncnn/models"),
            internal_scale: 4,
            final_scale: 2,
            tile_size: None,
            threads: "3:3:3".into(),
            vk_device: 0,
            jpeg_quality: 2,
            crush: "none".into(),
            brightness: String::new(),
            target_dar: "4:3".into(),
            allow_mixed: false,
            batch_size: None,
            skip_sync_fix: false,
        }
    }

    pub fn tile(&self) -> u32 {
        self.tile_size.unwrap_or(match self.backend {
            Backend::Vulkan => 400,
            Backend::Rocm => 0,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Vulkan => "vulkan",
            Backend::Rocm => "rocm",
        }
    }

    pub fn layout(&self) -> Layout {
        let stem = self
            .input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let work_dir = self.work_root.join(stem);
        Layout {
            segments_dir: work_dir.join("segments"),
            frames_dir: work_dir.join("frames"),
            upscaled_dir: work_dir.join("frames_up"),
            part_dir: work_dir.join("seg_part"),
            config_file: work_dir.join("run_config.txt"),
            pgid_file: work_dir.join("upscale.pgid"),
            concat_list: work_dir.join("segments.txt"),
            concat_video: work_dir.join("video_concat.mp4"),
            work_dir,
        }
    }
}

/// Where a job keeps its files. The layout is the scripts' (the GUI counts `segments/seg_*.mp4`
/// and the two frame directories), plus `seg_part/` where a segment is encoded before it is
/// renamed into `segments/`, so a killed encode can never be mistaken for a finished segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub work_dir: PathBuf,
    pub segments_dir: PathBuf,
    pub frames_dir: PathBuf,
    pub upscaled_dir: PathBuf,
    pub part_dir: PathBuf,
    pub config_file: PathBuf,
    pub pgid_file: PathBuf,
    pub concat_list: PathBuf,
    pub concat_video: PathBuf,
}

impl Layout {
    pub fn segment_name(i: u64) -> String {
        format!("seg_{i:03}.mp4")
    }

    pub fn segment_path(&self, i: u64) -> PathBuf {
        self.segments_dir.join(Self::segment_name(i))
    }

    pub fn part_path(&self, i: u64) -> PathBuf {
        self.part_dir.join(Self::segment_name(i))
    }
}

/// `ceil(duration)`, the script's `TOTAL_SECONDS`.
pub fn total_seconds(duration: f64) -> u64 {
    duration.ceil().max(0.0) as u64
}

pub fn segment_count(total: u64, segment_secs: u32) -> u64 {
    let s = u64::from(segment_secs.max(1));
    total.div_ceil(s)
}

/// Start and length in seconds of segment `i`; the last one is shortened to the source.
pub fn segment_bounds(i: u64, segment_secs: u32, total: u64) -> (u64, u64) {
    let s = u64::from(segment_secs);
    let start = i * s;
    let len = if start + s > total { total - start } else { s };
    (start, len)
}

/// Scale-math checks, with the scripts' messages.
pub fn validate_scales(internal: u32, final_: u32) -> Result<(), String> {
    if internal == 0 || final_ == 0 {
        return Err("INTERNAL_SCALE and FINAL_SCALE must be positive.".into());
    }
    if !internal.is_multiple_of(final_) {
        return Err(format!(
            "INTERNAL_SCALE must be evenly divisible by FINAL_SCALE (got {internal} and {final_})."
        ));
    }
    Ok(())
}

/// DAR-correct output size: `FINAL_H = src_h * FINAL_SCALE`, `FINAL_W = FINAL_H * num / den` made
/// even (H.264). An empty or colon-less `target_dar` falls back to plain pixel scaling.
pub fn output_size(src_w: u32, src_h: u32, final_scale: u32, target_dar: &str) -> (u32, u32) {
    let h = src_h * final_scale;
    if let Some((num, den)) = target_dar.split_once(':')
        && let (Ok(num), Ok(den)) = (num.parse::<u32>(), den.parse::<u32>())
        && den != 0
    {
        let w = h * num / den;
        (w + w % 2, h)
    } else {
        (src_w * final_scale, h)
    }
}

/// The scripts' `_resolve_brightness`: named levels, anything else as given.
pub fn resolve_brightness(b: &str) -> String {
    match b {
        "none" => "0",
        "low" => "0.02",
        "medium" => "0.05",
        "high" => "0.095",
        other => other,
    }
    .to_string()
}

/// The pre-filter chain (`PRE_VF`, or `BW_FILTER` for B&W): `hqdn3d`, the B&W `hue=s=0`, an
/// optional luma remap for `small`/`medium`/`heavy`, and `eq=brightness` unless it resolves to
/// exactly `0`.
pub fn pre_filter(variant: Variant, crush: &str, brightness: &str) -> Result<String, String> {
    let (remap_from, default_brightness) = match crush {
        "none" => (None, "0"),
        "small" => (Some((16, 239)), "0"),
        "medium" => (Some((50, 205)), "0.05"),
        "heavy" => (Some((70, 185)), "0.095"),
        other => {
            return Err(format!(
                "Unknown CRUSH preset '{other}' (expected: none|small|medium|heavy)"
            ));
        }
    };
    let mut chain = String::from("hqdn3d=3:2:4:3");
    if variant == Variant::Bw {
        chain.push_str(",hue=s=0");
    }
    if let Some((from, span)) = remap_from {
        chain.push_str(&format!(
            ",lutyuv=y='if(lt(val,{from}),0,min(255,(val-{from})*255/{span}))'"
        ));
    }
    let b = if brightness.is_empty() {
        default_brightness
    } else {
        brightness
    };
    let b = resolve_brightness(b);
    Ok(if b != "0" {
        format!("{chain},eq=brightness={b}")
    } else {
        chain
    })
}

/// `realesrgan-ncnn-vulkan` crashes on model names outside these families.
pub fn vulkan_supports(model: &str) -> bool {
    matches!(
        model,
        "realesrgan-x4plus" | "realesrgan-x4plus-anime" | "realesrnet-x4plus"
    ) || model.starts_with("realesr-animevideov3")
}

pub fn vulkan_model_error(model: &str) -> String {
    format!(
        "MODEL='{model}' is not supported by the vulkan backend (realesrgan-ncnn-vulkan crashes on unrecognized model names). Use UPSCALE_BACKEND=rocm for community VHS models (2x_VHS-Film, ToonVHS-1x, VHS-Sharpen-1x)."
    )
}

/// The `run_config.txt` text, byte for byte as the scripts write it: the keys in the scripts'
/// order and no trailing newline. `fps` is the probed `r_frame_rate` string.
pub fn fingerprint(spec: &JobSpec, fps: &str, filter: &str) -> String {
    let base = spec
        .input
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut lines = vec![
        format!("SCRIPT={}", spec.variant.script_name()),
        format!("INPUT_BASENAME={base}"),
        format!("SEG_SECONDS={}", spec.segment_secs),
        format!("CRF={}", spec.crf),
        format!("FPS={fps}"),
        format!("MODEL={}", spec.model),
        format!("MODELS_DIR={}", spec.models_dir.display()),
        format!("INTERNAL_SCALE={}", spec.internal_scale),
        format!("FINAL_SCALE={}", spec.final_scale),
        format!("JPEG_QUALITY={}", spec.jpeg_quality),
        format!("TILE_SIZE={}", spec.tile()),
        format!("THREADS={}", spec.threads),
        format!("VK_DEVICE_INDEX={}", spec.vk_device),
        format!("UPSCALE_BACKEND={}", spec.backend_name()),
        format!("PRESET={}", spec.preset),
        format!("{}={filter}", spec.variant.filter_key()),
    ];
    if spec.variant == Variant::Anime {
        // The anime script also records its decomb settings. Decomb is not ported, so they are
        // always the script's defaults, which keeps its work directories resumable.
        lines.push("DECOMB=0".into());
        lines.push("VS_TFF=1".into());
        lines.push("VS_DECOMB_PRESET=Fast".into());
    }
    lines.join("\n")
}

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// Frame extraction for one segment. B&W always filters and forces RGB (the upscaler needs it).
pub fn extract_args(
    spec: &JobSpec,
    layout: &Layout,
    start: u64,
    len: u64,
    filter: &str,
) -> Vec<OsString> {
    let mut a = args(&[
        "-y",
        "-ss",
        &start.to_string(),
        "-t",
        &len.to_string(),
        "-i",
    ]);
    a.push(spec.input.clone().into());
    a.push("-an".into());
    match spec.variant {
        Variant::Bw => {
            a.push("-vf".into());
            a.push(format!("{filter},format=rgb24").into());
        }
        _ if !filter.is_empty() => {
            a.push("-vf".into());
            a.push(filter.into());
        }
        _ => {}
    }
    a.extend(args(&["-qscale:v", &spec.jpeg_quality.to_string()]));
    a.push(layout.frames_dir.join("frame_%08d.jpg").into());
    a
}

pub fn upscale_args(spec: &JobSpec, layout: &Layout) -> Vec<OsString> {
    let mut a = args(&["-i"]);
    a.push(layout.frames_dir.clone().into());
    a.push("-o".into());
    a.push(layout.upscaled_dir.clone().into());
    a.extend(args(&["-s", &spec.internal_scale.to_string(), "-m"]));
    a.push(spec.models_dir.clone().into());
    a.extend(args(&[
        "-n",
        &spec.model,
        "-t",
        &spec.tile().to_string(),
        "-j",
        &spec.threads,
        "-g",
        &spec.vk_device.to_string(),
        "-f",
        "jpg",
    ]));
    a
}

/// Rebuild one segment's video from its upscaled frames, writing to `out`.
pub fn rebuild_args(
    spec: &JobSpec,
    layout: &Layout,
    fps: &str,
    size: (u32, u32),
    out: &Path,
) -> Vec<OsString> {
    let mut a = args(&["-y", "-framerate", fps, "-i"]);
    a.push(layout.upscaled_dir.join("frame_%08d.jpg").into());
    let tail = if spec.variant == Variant::Bw {
        ",format=yuv420p"
    } else {
        ""
    };
    a.extend(args(&[
        "-vf",
        &format!("scale={}:{}:flags=lanczos,setsar=1{tail}", size.0, size.1),
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        &spec.preset,
        "-crf",
        &spec.crf.to_string(),
        "-pix_fmt",
        "yuv420p",
    ]));
    a.push(out.into());
    a
}

/// `ffprobe` arguments that count a segment's video packets.
pub fn count_packets_args(file: &Path) -> Vec<OsString> {
    let mut a = args(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-count_packets",
        "-show_entries",
        "stream=nb_read_packets",
        "-of",
        "default=nk=1:nw=1",
    ]);
    a.push(file.into());
    a
}

pub fn probe_duration_args(file: &Path) -> Vec<OsString> {
    let mut a = args(&[
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "csv=p=0",
    ]);
    a.push(file.into());
    a
}

pub fn probe_stream_args(entry: &str, file: &Path) -> Vec<OsString> {
    let mut a = args(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        &format!("stream={entry}"),
        "-of",
        "csv=p=0",
    ]);
    a.push(file.into());
    a
}

pub fn probe_audio_args(file: &Path) -> Vec<OsString> {
    let mut a = args(&[
        "-v",
        "error",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=index",
        "-of",
        "csv=p=0",
    ]);
    a.push(file.into());
    a
}

pub fn concat_args(layout: &Layout) -> Vec<OsString> {
    let mut a = args(&["-y", "-f", "concat", "-safe", "0", "-i"]);
    a.push(layout.concat_list.clone().into());
    a.extend(args(&["-c", "copy"]));
    a.push(layout.concat_video.clone().into());
    a
}

/// Mux the source's first audio stream (AAC 160k) into the concatenated video, or just copy the
/// video when the source has no audio.
pub fn mux_args(spec: &JobSpec, layout: &Layout, has_audio: bool) -> Vec<OsString> {
    let mut a = args(&["-y", "-i"]);
    a.push(layout.concat_video.clone().into());
    if has_audio {
        a.push("-i".into());
        a.push(spec.input.clone().into());
        a.extend(args(&[
            "-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac", "-b:a", "160k",
        ]));
    } else {
        a.extend(args(&["-c", "copy"]));
    }
    a.push(spec.output.clone().into());
    a
}

/// A segment shorter than a third of the nominal length is suspect, except the last one.
pub fn is_short(duration: f64, segment_secs: u32) -> bool {
    let min = i64::from(segment_secs / 3);
    (duration.trunc() as i64) < min
}

/// Drift between the source and the summed segment durations as the scripts print it: percent of
/// the source, to one decimal, signed (negative when the segments are longer).
pub fn drift_pct(source: f64, total: f64) -> f64 {
    let pct = (source - total) / source * 100.0;
    (pct * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> JobSpec {
        JobSpec::defaults(
            Variant::Standard,
            Path::new("/in/Tape One.mkv"),
            Path::new("/out/Tape One.upscale.mkv"),
            Path::new("/work"),
        )
    }

    #[test]
    fn segment_math_matches_the_scripts() {
        assert_eq!(total_seconds(70.0), 70);
        assert_eq!(total_seconds(70.001), 71);
        assert_eq!(segment_count(70, 30), 3);
        assert_eq!(segment_count(60, 30), 2);
        assert_eq!(segment_bounds(0, 30, 70), (0, 30));
        assert_eq!(segment_bounds(2, 30, 70), (60, 10));
    }

    #[test]
    fn scale_validation_has_the_scripts_messages() {
        assert!(validate_scales(4, 2).is_ok());
        assert!(validate_scales(1, 1).is_ok());
        assert_eq!(
            validate_scales(4, 3).unwrap_err(),
            "INTERNAL_SCALE must be evenly divisible by FINAL_SCALE (got 4 and 3)."
        );
        assert!(validate_scales(0, 2).is_err());
    }

    #[test]
    fn output_size_is_dar_correct_and_even() {
        // NTSC 720x480 at 2x: 960 tall, 4:3 wide = 1280.
        assert_eq!(output_size(720, 480, 2, "4:3"), (1280, 960));
        assert_eq!(output_size(720, 480, 1, "4:3"), (640, 480));
        // Odd widths are rounded up to even.
        assert_eq!(output_size(10, 3, 1, "4:3"), (4, 3));
        assert_eq!(output_size(720, 480, 2, ""), (1440, 960));
        assert_eq!(output_size(720, 480, 2, "garbage"), (1440, 960));
    }

    #[test]
    fn pre_filter_presets() {
        let f = |v, c, b| pre_filter(v, c, b).unwrap();
        assert_eq!(f(Variant::Standard, "none", ""), "hqdn3d=3:2:4:3");
        assert_eq!(f(Variant::Standard, "none", "none"), "hqdn3d=3:2:4:3");
        assert_eq!(
            f(Variant::Standard, "medium", ""),
            "hqdn3d=3:2:4:3,lutyuv=y='if(lt(val,50),0,min(255,(val-50)*255/205))',eq=brightness=0.05"
        );
        assert_eq!(
            f(Variant::Standard, "heavy", "high"),
            "hqdn3d=3:2:4:3,lutyuv=y='if(lt(val,70),0,min(255,(val-70)*255/185))',eq=brightness=0.095"
        );
        assert_eq!(
            f(Variant::Standard, "small", ""),
            "hqdn3d=3:2:4:3,lutyuv=y='if(lt(val,16),0,min(255,(val-16)*255/239))'"
        );
        assert_eq!(f(Variant::Bw, "none", ""), "hqdn3d=3:2:4:3,hue=s=0");
        assert_eq!(
            f(Variant::Standard, "none", "0.03"),
            "hqdn3d=3:2:4:3,eq=brightness=0.03"
        );
        // The script compares the resolved brightness with the string "0", so "0.0" adds a filter.
        assert_eq!(
            f(Variant::Standard, "none", "0.0"),
            "hqdn3d=3:2:4:3,eq=brightness=0.0"
        );
        assert!(pre_filter(Variant::Standard, "bogus", "").is_err());
    }

    #[test]
    fn fingerprint_is_the_scripts_text_with_no_trailing_newline() {
        let mut s = spec();
        s.models_dir = PathBuf::from("/m");
        let f = fingerprint(&s, "30000/1001", "hqdn3d=3:2:4:3");
        let want = "SCRIPT=vhs_upscale.sh\nINPUT_BASENAME=Tape One.mkv\nSEG_SECONDS=30\nCRF=21\n\
            FPS=30000/1001\nMODEL=realesrgan-x4plus\nMODELS_DIR=/m\nINTERNAL_SCALE=4\nFINAL_SCALE=2\n\
            JPEG_QUALITY=2\nTILE_SIZE=400\nTHREADS=3:3:3\nVK_DEVICE_INDEX=0\nUPSCALE_BACKEND=vulkan\n\
            PRESET=veryfast\nPRE_VF=hqdn3d=3:2:4:3";
        assert_eq!(f, want);
        assert!(!f.ends_with('\n'));
        // ROCm defaults to tile size 0; anime and B&W change the script and key names.
        s.backend = Backend::Rocm;
        assert!(fingerprint(&s, "25/1", "x").contains("TILE_SIZE=0\n"));
        s.variant = Variant::Bw;
        let bw = fingerprint(&s, "25/1", "x");
        assert!(bw.starts_with("SCRIPT=vhs_upscale_bw.sh\n") && bw.ends_with("BW_FILTER=x"));
        s.variant = Variant::Anime;
        assert!(
            fingerprint(&s, "25/1", "x")
                .ends_with("PRE_VF=x\nDECOMB=0\nVS_TFF=1\nVS_DECOMB_PRESET=Fast")
        );
    }

    #[test]
    fn layout_follows_the_scripts_work_dir() {
        let l = spec().layout();
        assert_eq!(l.work_dir, PathBuf::from("/work/Tape One"));
        assert_eq!(
            l.segment_path(7),
            PathBuf::from("/work/Tape One/segments/seg_007.mp4")
        );
        assert_eq!(
            l.part_path(7),
            PathBuf::from("/work/Tape One/seg_part/seg_007.mp4")
        );
        assert_eq!(l.pgid_file, PathBuf::from("/work/Tape One/upscale.pgid"));
    }

    fn joined(a: &[OsString]) -> String {
        a.iter()
            .map(|x| x.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn command_arguments_are_the_scripts() {
        let s = spec();
        let l = s.layout();
        assert_eq!(
            joined(&extract_args(&s, &l, 30, 30, "hqdn3d=3:2:4:3")),
            "-y -ss 30 -t 30 -i /in/Tape One.mkv -an -vf hqdn3d=3:2:4:3 -qscale:v 2 /work/Tape One/frames/frame_%08d.jpg"
        );
        assert_eq!(
            joined(&upscale_args(&s, &l)).split(" -m ").next().unwrap(),
            "-i /work/Tape One/frames -o /work/Tape One/frames_up -s 4"
        );
        assert!(
            joined(&upscale_args(&s, &l))
                .ends_with("-n realesrgan-x4plus -t 400 -j 3:3:3 -g 0 -f jpg")
        );
        assert_eq!(
            joined(&rebuild_args(
                &s,
                &l,
                "30000/1001",
                (1280, 960),
                Path::new("/p/seg_000.mp4")
            )),
            "-y -framerate 30000/1001 -i /work/Tape One/frames_up/frame_%08d.jpg \
             -vf scale=1280:960:flags=lanczos,setsar=1 -an -c:v libx264 -preset veryfast -crf 21 \
             -pix_fmt yuv420p /p/seg_000.mp4"
        );
        assert_eq!(
            joined(&mux_args(&s, &l, true)),
            "-y -i /work/Tape One/video_concat.mp4 -i /in/Tape One.mkv -map 0:v:0 -map 1:a:0 \
             -c:v copy -c:a aac -b:a 160k /out/Tape One.upscale.mkv"
        );
        assert_eq!(
            joined(&mux_args(&s, &l, false)),
            "-y -i /work/Tape One/video_concat.mp4 -c copy /out/Tape One.upscale.mkv"
        );
    }

    #[test]
    fn bw_forces_rgb_frames_and_a_pixel_format_in_the_rebuild_filter() {
        let mut s = spec();
        s.variant = Variant::Bw;
        let l = s.layout();
        assert!(
            joined(&extract_args(&s, &l, 0, 30, "hqdn3d=3:2:4:3,hue=s=0"))
                .contains("-vf hqdn3d=3:2:4:3,hue=s=0,format=rgb24 ")
        );
        assert!(
            joined(&rebuild_args(
                &s,
                &l,
                "25/1",
                (640, 480),
                Path::new("/p/s.mp4")
            ))
            .contains("scale=640:480:flags=lanczos,setsar=1,format=yuv420p -an")
        );
    }

    #[test]
    fn validation_helpers_match_the_scripts_rules() {
        // min = segment_secs / 3 (integer): 10 s for 30 s segments; the integer part is compared.
        assert!(is_short(9.99, 30));
        assert!(!is_short(10.0, 30));
        assert_eq!(drift_pct(100.0, 98.0), 2.0);
        assert_eq!(drift_pct(100.0, 101.0), -1.0);
        assert_eq!(drift_pct(70.0, 69.97), 0.0);
    }

    #[test]
    fn the_job_file_round_trips_through_toml() {
        let mut s = spec();
        s.backend = Backend::Rocm;
        s.batch_size = Some(2);
        s.tile_size = None;
        s.brightness = "low".into();
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<JobSpec>(&text).unwrap(), s);
        s.tile_size = Some(200);
        s.variant = Variant::Anime;
        assert_eq!(
            toml::from_str::<JobSpec>(&toml::to_string(&s).unwrap()).unwrap(),
            s
        );
    }

    #[test]
    fn variant_defaults() {
        assert_eq!(Variant::Anime.default_model(), "realesrgan-x4plus-anime");
        assert_eq!(Variant::Bw.default_model(), "realesrgan-x4plus");
        assert_eq!(Variant::Anime.max_drift_pct(), 8.0);
        assert_eq!(Variant::Standard.max_drift_pct(), 2.0);
        assert!(vulkan_supports("realesr-animevideov3-x2"));
        assert!(!vulkan_supports("2x_VHS-Film"));
    }
}
