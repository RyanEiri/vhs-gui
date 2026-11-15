//! Native port of `vhs_viewer_encode.sh`: the Plex/viewer derivative (H.264 + AAC in MKV).
//!
//! One ffmpeg call. The source height picks the preset, exactly as the script's auto mode does:
//!   * height <= 576 (SD/VHS): x264 CRF 18, `fast`, main@4.0, scaled to 640x480 (Lanczos), 30000/1001
//!     (a 24 fps film master keeps its own rate and a 25 fps source stays 25: see [`sd_cfr`])
//!   * taller (HD):            x264 CRF 20, `medium`, high@4.1, no scaling, CFR from the probed rate
//!
//! Audio is AAC-LC 320k stereo at 48 kHz. Output is `captures/viewer/<stem>.viewer.mkv`; an existing
//! output is left alone (the script's default, no `FORCE`).
//!
//! Not ported: the script's `MODE`/`DEINTERLACE`/`V_*`/`A_*` environment overrides and its 2-pass ABR
//! branch. The GUI never set them, and ABR is unreachable unless `V_CRF` is emptied, which the GUI
//! cannot do. `DEINTERLACE` defaulted to `off`, so no `bwdif` either.
//!
//! The argument list is built by a pure function ([`ffmpeg_args`]) so it can be compared with what
//! the script actually passes to ffmpeg (see the ignored `matches_the_bash_script` test).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::crop::Crop;
use crate::pipeline::PipelineJob;
use crate::tools::{self, Tool};

/// Sources this tall or shorter get the SD/VHS preset (the script's `-le 576`).
const SD_MAX_HEIGHT: u32 = 576;

const AUDIO_FILTER: &str = "aresample=async=1:first_pts=0,asetpts=N/SR/TB";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Vhs,
    Hd,
}

/// What ffprobe says about the first video stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub width: u32,
    pub height: u32,
    /// `avg_frame_rate` as a float; 0.0 when the stream reports none.
    pub fps: f64,
}

/// Everything that differs between the two presets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub preset: Preset,
    pub x264_preset: &'static str,
    pub profile: &'static str,
    pub level: &'static str,
    pub crf: u32,
    /// Output size for `scale=` (640x480), or none to keep the source size.
    pub scale: Option<(u32, u32)>,
    /// Letterbox crop applied before the scale, if the file has one.
    pub crop: Option<Crop>,
    /// Constant frame rate passed to `-r`.
    pub cfr: String,
}

/// The script's `pick_cfr`: snap a probed rate to a standard CFR value, else the nearest integer.
pub fn pick_cfr(fps: f64) -> String {
    // The script prints the probed rate with six decimals before comparing.
    let f = (fps * 1e6).round() / 1e6;
    if f > 29.0 && f < 30.2 {
        "30000/1001".into()
    } else if f > 59.0 && f < 60.2 {
        "60000/1001".into()
    } else if f > 23.5 && f < 24.2 {
        "24000/1001".into()
    } else if f > 49.0 && f < 50.2 {
        "50".into()
    } else {
        let n = (f + 0.5).trunc() as i64;
        (if n < 1 { 30 } else { n }).to_string()
    }
}

/// The constant rate for an SD source. The script forced 30000/1001 on everything, which gave a
/// 24 fps film master (a VDecimate output) a repeated frame in every five and made any upscale of
/// the copy 29.97 fps with those repeats. Film rates and 25 fps now keep their own rate; every
/// other SD rate (NTSC video, or an unreadable one) stays at 30000/1001 as before.
pub fn sd_cfr(fps: f64) -> String {
    if (fps - 24000.0 / 1001.0).abs() < 0.005 {
        "24000/1001".into()
    } else if (23.9..24.1).contains(&fps) {
        "24".into()
    } else if (24.9..25.1).contains(&fps) {
        "25".into()
    } else {
        "30000/1001".into()
    }
}

pub fn plan(src: &Source) -> Plan {
    if src.height <= SD_MAX_HEIGHT {
        Plan {
            preset: Preset::Vhs,
            x264_preset: "fast",
            profile: "main",
            level: "4.0",
            crf: 18,
            scale: Some((640, 480)),
            crop: None,
            cfr: sd_cfr(src.fps),
        }
    } else {
        Plan {
            preset: Preset::Hd,
            x264_preset: "medium",
            profile: "high",
            level: "4.1",
            crf: 20,
            scale: None,
            crop: None,
            cfr: pick_cfr(src.fps),
        }
    }
}

impl Plan {
    /// This plan with a letterbox crop. The VHS preset's 640x480 becomes the same share of the
    /// frame, so a 720x276 picture cut from a 4:3 frame is scaled to 640x276 and keeps its shape;
    /// the HD preset keeps the cropped size.
    pub fn with_crop(mut self, src: &Source, crop: Crop) -> Plan {
        if let Some((w, h)) = self.scale {
            let even = |v: u32| v + v % 2;
            self.scale = Some((
                even(w * crop.w / src.width.max(1)),
                even(h * crop.h / src.height.max(1)),
            ));
        }
        self.crop = Some(crop);
        self
    }
}

/// `-vf` chain: optional crop, optional Lanczos scale, square pixels, 4:2:0.
pub fn video_filter(plan: &Plan) -> String {
    let mut parts = Vec::new();
    if let Some(c) = &plan.crop {
        parts.push(c.filter());
    }
    if let Some((w, h)) = plan.scale {
        parts.push(format!("scale={w}:{h}:flags=lanczos"));
    }
    parts.push("setsar=1".to_string());
    parts.push("format=yuv420p".to_string());
    parts.join(",")
}

/// The ffmpeg arguments, in the script's order.
pub fn ffmpeg_args(input: &Path, output: &Path, plan: &Plan) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |s: &str| a.push(s.into());
    for s in ["-hide_banner", "-nostdin", "-y", "-fflags", "+genpts", "-i"] {
        push(s);
    }
    a.push(input.into());
    let mut push = |s: &str| a.push(s.into());
    for s in ["-map", "0:v:0", "-map", "0:a:0", "-vf"] {
        push(s);
    }
    a.push(video_filter(plan).into());
    let mut push = |s: &str| a.push(s.into());
    for s in ["-fps_mode", "cfr", "-r", plan.cfr.as_str()] {
        push(s);
    }
    for s in [
        "-c:v",
        "libx264",
        "-preset",
        plan.x264_preset,
        "-profile:v",
        plan.profile,
        "-level:v",
        plan.level,
        "-crf",
    ] {
        push(s);
    }
    a.push(plan.crf.to_string().into());
    let mut push = |s: &str| a.push(s.into());
    for s in [
        "-c:a",
        "aac",
        "-profile:a",
        "aac_low",
        "-b:a",
        "320k",
        "-aac_coder",
        "twoloop",
        "-cutoff",
        "20000",
        "-af",
        AUDIO_FILTER,
        "-ar",
        "48000",
        "-ac",
        "2",
    ] {
        push(s);
    }
    a.push(output.into());
    a
}

/// Parse `ffprobe -show_entries stream=width,height,avg_frame_rate -of default=nw=1:nk=1`:
/// three lines, width, height and a rate such as `30000/1001`.
pub fn parse_probe(stdout: &str) -> Option<Source> {
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let width = lines.next()?.parse().ok()?;
    let height = lines.next()?.parse().ok()?;
    let rate = lines.next()?;
    let fps = match rate.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            if d > 0.0 { n / d } else { 0.0 }
        }
        None => rate.parse().ok()?,
    };
    Some(Source { width, height, fps })
}

fn probe_source(input: &Path) -> anyhow::Result<Source> {
    let out = tools::command(Tool::Ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(input)
        .output()?;
    parse_probe(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| anyhow::anyhow!("could not read the video size and frame rate"))
}

/// `captures/viewer/<stem>.viewer.mkv`, where the stem drops a trailing `.mkv` only (the script's
/// `stem_of`).
pub fn output_path(input: &Path, cfg: &Config) -> PathBuf {
    let name = input.file_name().and_then(|s| s.to_str()).unwrap_or("out");
    let stem = name.strip_suffix(".mkv").unwrap_or(name);
    cfg.viewer_dir().join(format!("{stem}.viewer.mkv"))
}

pub enum Launch {
    /// The encode is running; the path is where its output will appear.
    Started(Box<PipelineJob>, PathBuf),
    /// The viewer copy already exists and was left alone.
    Exists(PathBuf),
}

pub fn launch(
    input: &Path,
    cfg: &Config,
    label: String,
    crop: Option<Crop>,
) -> anyhow::Result<Launch> {
    tools::require(Tool::Ffprobe)?;
    tools::require(Tool::Ffmpeg)?;
    if !input.is_file() {
        anyhow::bail!("input not found: {}", input.display());
    }
    let output = output_path(input, cfg);
    if output.exists() {
        return Ok(Launch::Exists(output));
    }
    fs::create_dir_all(cfg.viewer_dir())?;

    let source = probe_source(input)?;
    let mut plan = plan(&source);
    if let Some(c) = crop {
        c.validate(source.width, source.height)
            .map_err(|e| anyhow::anyhow!("invalid letterbox crop: {e}"))?;
        plan = plan.with_crop(&source, c);
    }
    let mut cmd = tools::command(Tool::Ffmpeg);
    cmd.args(ffmpeg_args(input, &output, &plan));
    let job = PipelineJob::start_native_single(label, input, cmd, &cfg.log_dir())?;
    Ok(Launch::Started(Box::new(job), output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    fn src(w: u32, h: u32, fps: f64) -> Source {
        Source {
            width: w,
            height: h,
            fps,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// A scratch directory that is removed when dropped, so a failing assertion cannot leave it behind.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sd_sources_get_the_vhs_preset_at_the_576_line_boundary() {
        for (w, h) in [(720, 480), (640, 480), (720, 576), (768, 576)] {
            let p = plan(&src(w, h, 25.0));
            assert_eq!(p.preset, Preset::Vhs, "{w}x{h}");
            assert_eq!(p.crf, 18);
            assert_eq!(p.scale, Some((640, 480)));
            // A 25 fps source keeps its rate; 29.97 and unknown rates are covered in
            // `sd_film_and_pal_sources_keep_their_own_rate`.
            assert_eq!(p.cfr, "25");
        }
        let hd = plan(&src(720, 577, 25.0));
        assert_eq!(hd.preset, Preset::Hd);
        assert_eq!((hd.crf, hd.scale, hd.x264_preset), (20, None, "medium"));
    }

    const LETTERBOX: Crop = Crop {
        w: 720,
        h: 276,
        x: 0,
        y: 98,
    };

    #[test]
    fn a_letterbox_crop_goes_before_the_scale_and_keeps_the_picture_shape() {
        let s = src(720, 480, 29.97);
        let p = plan(&s).with_crop(&s, LETTERBOX);
        assert_eq!(p.scale, Some((640, 276)));
        assert_eq!(
            video_filter(&p),
            "crop=720:276:0:98,scale=640:276:flags=lanczos,setsar=1,format=yuv420p"
        );
        // Everything else about the VHS preset is unchanged.
        let plain = plan(&s);
        assert_eq!(
            (p.crf, p.x264_preset, p.profile, p.level, &p.cfr),
            (
                plain.crf,
                plain.x264_preset,
                plain.profile,
                plain.level,
                &plain.cfr
            )
        );
    }

    #[test]
    fn an_hd_source_is_cropped_but_not_scaled() {
        let s = src(1920, 1080, 23.976);
        let c = Crop {
            w: 1920,
            h: 800,
            x: 0,
            y: 140,
        };
        let p = plan(&s).with_crop(&s, c);
        assert_eq!(p.scale, None);
        assert_eq!(
            video_filter(&p),
            "crop=1920:800:0:140,setsar=1,format=yuv420p"
        );
    }

    #[test]
    fn a_narrower_crop_scales_in_proportion_and_stays_even() {
        let s = src(720, 480, 29.97);
        let c = Crop {
            w: 640,
            h: 276,
            x: 40,
            y: 98,
        };
        let (w, h) = plan(&s).with_crop(&s, c).scale.unwrap();
        assert_eq!((w, h), (568, 276)); // 640 * 640 / 720, rounded down by integer division
        assert!(w.is_multiple_of(2) && h.is_multiple_of(2));
    }

    #[test]
    fn sd_film_and_pal_sources_keep_their_own_rate() {
        for (fps, want) in [
            (24.0, "24"),               // a VDecimate master
            (23.976_024, "24000/1001"), // film rate as NTSC
            (23.98, "24000/1001"),      // the average rate of a slightly variable file
            (25.0, "25"),               // PAL
            (29.970_03, "30000/1001"),  // NTSC video, as the script always did
            (30.0, "30000/1001"),
            (0.0, "30000/1001"),   // no rate reported
            (59.94, "30000/1001"), // nothing else is guessed at
        ] {
            assert_eq!(sd_cfr(fps), want, "{fps}");
        }
    }

    #[test]
    fn a_24_fps_sd_master_is_encoded_at_24_not_padded_to_29_97() {
        let p = plan(&src(720, 480, 24.0));
        assert_eq!(p.preset, Preset::Vhs);
        let args = strings(&ffmpeg_args(
            Path::new("/in/a.mkv"),
            Path::new("/out/a.mkv"),
            &p,
        ))
        .join(" ");
        assert!(args.contains("-fps_mode cfr -r 24 "), "{args}");
        assert!(!args.contains("30000/1001"), "{args}");
        // The rest of the VHS preset is unchanged.
        assert_eq!(
            (p.crf, p.x264_preset, p.scale),
            (18, "fast", Some((640, 480)))
        );
    }

    #[test]
    fn pick_cfr_matches_the_scripts_table() {
        let cases = [
            (29.97003, "30000/1001"),
            (30.0, "30000/1001"),
            (59.94006, "60000/1001"),
            (60.0, "60000/1001"),
            (23.976024, "24000/1001"),
            (24.0, "24000/1001"),
            (25.0, "25"),
            (50.0, "50"),
            (15.0, "15"),
            (14.6, "15"),
            (0.0, "30"),
            (0.4, "30"),
        ];
        for (fps, want) in cases {
            assert_eq!(pick_cfr(fps), want, "{fps}");
        }
    }

    #[test]
    fn video_filter_chain() {
        assert_eq!(
            video_filter(&plan(&src(720, 480, 29.97))),
            "scale=640:480:flags=lanczos,setsar=1,format=yuv420p"
        );
        assert_eq!(
            video_filter(&plan(&src(1920, 1080, 59.94))),
            "setsar=1,format=yuv420p"
        );
    }

    #[test]
    fn sd_arguments_are_exactly_the_scripts() {
        let args = ffmpeg_args(
            Path::new("/in/a.mkv"),
            Path::new("/out/a.viewer.mkv"),
            &plan(&src(720, 480, 29.97)),
        );
        let want = "-hide_banner -nostdin -y -fflags +genpts -i /in/a.mkv -map 0:v:0 -map 0:a:0 \
            -vf scale=640:480:flags=lanczos,setsar=1,format=yuv420p -fps_mode cfr -r 30000/1001 \
            -c:v libx264 -preset fast -profile:v main -level:v 4.0 -crf 18 \
            -c:a aac -profile:a aac_low -b:a 320k -aac_coder twoloop -cutoff 20000 \
            -af aresample=async=1:first_pts=0,asetpts=N/SR/TB -ar 48000 -ac 2 /out/a.viewer.mkv";
        assert_eq!(strings(&args).join(" "), want);
    }

    #[test]
    fn hd_arguments_use_the_probed_rate_and_keep_the_size() {
        let args = strings(&ffmpeg_args(
            Path::new("/in/b.mkv"),
            Path::new("/out/b.viewer.mkv"),
            &plan(&src(1920, 1080, 23.976)),
        ))
        .join(" ");
        assert!(args.contains("-vf setsar=1,format=yuv420p "), "{args}");
        assert!(args.contains("-r 24000/1001 "), "{args}");
        assert!(
            args.contains("-preset medium -profile:v high -level:v 4.1 -crf 20 "),
            "{args}"
        );
    }

    #[test]
    fn probe_output_parsing() {
        assert_eq!(
            parse_probe("720\n480\n30000/1001\n"),
            Some(src(720, 480, 30000.0 / 1001.0))
        );
        assert_eq!(parse_probe("1920\n1080\n25/1\n").unwrap().fps, 25.0);
        // ffprobe reports 0/0 for a stream without a rate.
        assert_eq!(parse_probe("1920\n1080\n0/0\n").unwrap().fps, 0.0);
        assert_eq!(parse_probe(""), None);
        assert_eq!(parse_probe("720\nN/A\n30/1\n"), None);
    }

    #[test]
    fn output_path_drops_only_a_trailing_mkv() {
        let cfg = Config::default();
        let dir = cfg.viewer_dir();
        assert_eq!(
            output_path(Path::new("/x/EDIT_MASTER-A_VD.mkv"), &cfg),
            dir.join("EDIT_MASTER-A_VD.viewer.mkv")
        );
        assert_eq!(
            output_path(Path::new("/x/clip.v2.mkv"), &cfg),
            dir.join("clip.v2.viewer.mkv")
        );
        assert_eq!(
            output_path(Path::new("/x/noext"), &cfg),
            dir.join("noext.viewer.mkv")
        );
    }

    /// The real `launch` path with real ffmpeg: generate a short SD clip, encode it, check the
    /// result, and check a second launch leaves the existing copy alone. Writes only under a temp
    /// directory. Not run by default (`cargo test -- --ignored`).
    /// Encodes a 3 s synthetic 720x480 FFV1 clip at `rate` through `launch`, waits for it, and
    /// returns the scratch dir guard, config, input and output paths.
    fn encode_synthetic(name: &str, rate: &str) -> (TempDir, Config, PathBuf, PathBuf) {
        let guard = TempDir::new(name);
        let dir = guard.0.clone();
        let cfg = Config {
            videos_dir: dir.clone(),
            ..Config::default()
        };

        let input = dir.join("clip_VD.mkv");
        let made = tools::command(Tool::Ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size=720x480:rate={rate}:duration=3"))
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
            .args(["-c:v", "ffv1", "-c:a", "pcm_s16le", "-shortest"])
            .arg(&input)
            .status()
            .unwrap();
        assert!(made.success());

        let Launch::Started(mut job, output) =
            launch(&input, &cfg, "smoke viewer encode".into(), None).unwrap()
        else {
            panic!("expected the encode to start");
        };
        assert_eq!(output, dir.join("captures/viewer/clip_VD.viewer.mkv"));
        for _ in 0..600 {
            job.poll();
            if job.done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(job.done, "encode did not finish in 60 s");
        assert_eq!(job.exit_ok, Some(true));
        assert!(output.is_file());
        (guard, cfg, input, output)
    }

    fn probe_output(output: &Path, sel: &str, entries: &str) -> String {
        let out = tools::command(Tool::Ffprobe)
            .args(["-v", "error", "-count_frames", "-select_streams", sel])
            .args(["-show_entries", entries, "-of", "csv=p=0"])
            .arg(output)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A 24 fps film master (a VDecimate output) keeps 24 fps and its 72 frames instead of being
    /// padded to 29.97 with repeated frames.
    #[test]
    #[ignore]
    fn a_24_fps_master_is_not_padded_to_29_97() {
        let (_guard, _cfg, _input, output) = encode_synthetic("vhs-gui-viewer-24", "24");
        assert_eq!(
            probe_output(&output, "v:0", "stream=r_frame_rate,nb_read_frames"),
            "24/1,72"
        );
    }

    #[test]
    #[ignore]
    fn end_to_end_smoke() {
        let (_guard, cfg, input, output) = encode_synthetic("vhs-gui-viewer-e2e", "30000/1001");

        assert_eq!(
            probe_output(&output, "v:0", "stream=codec_name,width,height,pix_fmt"),
            "h264,640,480,yuv420p"
        );
        assert_eq!(
            probe_output(&output, "a:0", "stream=codec_name,sample_rate,channels"),
            "aac,48000,2"
        );

        assert!(matches!(
            launch(&input, &cfg, "again".into(), None),
            Ok(Launch::Exists(_))
        ));
    }

    /// Runs the real `vhs_viewer_encode.sh` with stub ffprobe/ffmpeg binaries that record what they
    /// were called with, and checks the Rust argument list is identical. Needs the sibling vhs-cli
    /// checkout, so it is not run by default (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn matches_the_bash_script() {
        let script = crate::config::vhs_cli_dir().join("vhs_viewer_encode.sh");
        assert!(script.is_file(), "script missing: {}", script.display());

        let guard = TempDir::new("vhs-gui-viewer");
        let dir = guard.0.clone();
        let stub = |name: &str, body: &str| {
            let p = dir.join(name);
            fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let ffprobe = stub("ffprobe", "cat \"$(dirname \"$0\")/probe.txt\"");
        let ffmpeg = stub(
            "ffmpeg",
            "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv.txt\"",
        );
        let input = dir.join("in.mkv");
        fs::write(&input, "").unwrap();

        let cases = [
            ("720", "480", "30000/1001"),
            ("640", "480", "30000/1001"),
            ("720", "480", "24/1"),
            ("720", "480", "24000/1001"),
            ("720", "576", "25/1"),
            ("768", "576", "25/1"),
            ("720", "577", "30000/1001"),
            ("1280", "720", "60000/1001"),
            ("1920", "1080", "24000/1001"),
            ("1920", "1080", "30000/1001"),
            ("1920", "1080", "50/1"),
            ("1920", "1080", "25/1"),
            ("1920", "1080", "15/1"),
            ("1920", "1080", "0/0"),
        ];
        for (i, (w, h, rate)) in cases.iter().enumerate() {
            let probe = format!("{w}\n{h}\n{rate}\n");
            fs::write(dir.join("probe.txt"), &probe).unwrap();
            let output = dir.join(format!("out{i}.viewer.mkv"));
            let status = Command::new("bash")
                .arg(&script)
                .arg(&input)
                .arg(&output)
                .env("FFMPEG_BIN", &ffmpeg)
                .env("FFPROBE_BIN", &ffprobe)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "script failed for {w}x{h} {rate}");

            let mut from_script: Vec<String> = fs::read_to_string(dir.join("argv.txt"))
                .unwrap()
                .lines()
                .map(String::from)
                .collect();
            let source = parse_probe(&probe).unwrap();
            let p = plan(&source);
            // The one deliberate difference: the script forces 30000/1001 on every SD source, but
            // film and 25 fps sources keep their own rate (`sd_cfr`). Everything else must match.
            if p.preset == Preset::Vhs
                && p.cfr != "30000/1001"
                && let Some(i) = from_script.iter().position(|a| a == "-r")
            {
                from_script[i + 1] = p.cfr.clone();
            }
            let ours = strings(&ffmpeg_args(&input, &output, &p));
            assert_eq!(ours, from_script, "{w}x{h} @ {rate}");
        }
    }
}
