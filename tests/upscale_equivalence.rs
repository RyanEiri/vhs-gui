//! The real `vhs_upscale.sh` and the native `vhs-gui upscale-worker` on the same clip, with the real
//! ffmpeg and the real ROCm upscaler, and their outputs compared. Needs the GPU, `~/bin/realesrgan-rocm`
//! and the sibling vhs-cli checkout, and takes a minute or two, so it is ignored by default:
//! `cargo test --test upscale_equivalence -- --ignored --nocapture`.
//! Run it with no other upscale running.

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap())
}

fn run(mut cmd: Command, what: &str) {
    let status = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .status()
        .unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(status.success(), "{what} failed: {status}");
}

fn ffprobe(args: &[&str], file: &Path) -> String {
    let out = Command::new("/usr/bin/ffprobe")
        .args(["-v", "error"])
        .args(args)
        .arg(file)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// md5 of every decoded video frame (or audio packet), in order.
fn stream_md5(file: &Path, video: bool) -> String {
    let (map, fmt) = if video {
        ("0:v:0", "framemd5")
    } else {
        ("0:a:0", "md5")
    };
    let out = Command::new("/usr/bin/ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", map, "-f", fmt, "-"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn psnr(a: &Path, b: &Path) -> String {
    let out = Command::new("/usr/bin/ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(a)
        .arg("-i")
        .arg(b)
        .args(["-lavfi", "psnr", "-f", "null", "-"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .rev()
        .find(|l| l.contains("PSNR"))
        .unwrap_or("no PSNR line")
        .to_string()
}

#[test]
#[ignore]
fn the_native_engine_matches_vhs_upscale_sh_on_the_gpu() {
    let script = home().join("Videos/vhs-cli/vhs_upscale.sh");
    assert!(script.is_file(), "script missing: {}", script.display());
    assert!(
        home().join("bin/realesrgan-rocm").is_file(),
        "~/bin/realesrgan-rocm missing"
    );

    let root = std::env::temp_dir().join(format!("vhs-gui-equiv-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for d in ["a", "b", "workA", "workB"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    // The same input file name in both runs (the work directory is named after it).
    let clip_a = root.join("a/Tape.mkv");
    let clip_b = root.join("b/Tape.mkv");
    run(
        {
            let mut c = Command::new("/usr/bin/ffmpeg");
            c.args(["-hide_banner", "-loglevel", "error", "-y"])
                .args([
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=720x480:rate=30000/1001:duration=8",
                ])
                .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=8"])
                .args(["-c:v", "ffv1", "-c:a", "pcm_s16le", "-shortest"])
                .arg(&clip_a);
            c
        },
        "making the test clip",
    );
    fs::copy(&clip_a, &clip_b).unwrap();

    let out_a = root.join("a/Tape.upscale.mkv");
    let out_b = root.join("b/Tape.upscale.mkv");
    let path_env = format!("{}:/usr/bin:/bin", home().join("bin").display());

    // 1. The script, with the settings the GUI would pass for the ROCm backend.
    let mut script_cmd = Command::new("bash");
    script_cmd
        .arg(&script)
        .arg(&clip_a)
        .arg(&out_a)
        .args(["4", "21"])
        .env_clear()
        .env("PATH", &path_env)
        .env("HOME", home())
        .env("WORK_ROOT", root.join("workA"))
        .env("UPSCALE_BACKEND", "rocm")
        .env("MODEL", "realesrgan-x4plus")
        .env("INTERNAL_SCALE", "4")
        .env("FINAL_SCALE", "2")
        .env("CRUSH", "none")
        .env("BATCH_SIZE", "2");
    run(script_cmd, "vhs_upscale.sh");

    // 2. The native worker, same settings.
    let job = root.join("job.toml");
    fs::write(
        &job,
        format!(
            "variant = \"standard\"\ninput = \"{}\"\noutput = \"{}\"\nwork_root = \"{}\"\n\
             segment_secs = 4\ncrf = 21\npreset = \"veryfast\"\nbackend = \"rocm\"\n\
             model = \"realesrgan-x4plus\"\nmodels_dir = \"{}\"\ninternal_scale = 4\nfinal_scale = 2\n\
             threads = \"3:3:3\"\nvk_device = 0\njpeg_quality = 2\ncrush = \"none\"\nbrightness = \"\"\n\
             target_dar = \"4:3\"\nallow_mixed = false\nbatch_size = 2\n",
            clip_b.display(),
            out_b.display(),
            root.join("workB").display(),
            home().join("opt/realesrgan-ncnn/models").display()
        ),
    )
    .unwrap();
    let mut worker = Command::new(env!("CARGO_BIN_EXE_vhs-gui"));
    worker
        .arg("upscale-worker")
        .arg(&job)
        .env("PATH", &path_env);
    run(worker, "vhs-gui upscale-worker");

    // 3. Compare.
    for f in [&out_a, &out_b] {
        assert!(f.is_file(), "missing output {}", f.display());
    }
    let info = |f: &Path| {
        (
            ffprobe(
                &[
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=codec_name,width,height,pix_fmt,r_frame_rate",
                    "-of",
                    "csv=p=0",
                ],
                f,
            ),
            ffprobe(
                &[
                    "-select_streams",
                    "a:0",
                    "-show_entries",
                    "stream=codec_name,sample_rate,channels",
                    "-of",
                    "csv=p=0",
                ],
                f,
            ),
            ffprobe(
                &[
                    "-select_streams",
                    "v:0",
                    "-count_packets",
                    "-show_entries",
                    "stream=nb_read_packets",
                    "-of",
                    "csv=p=0",
                ],
                f,
            ),
            ffprobe(&["-show_entries", "format=duration", "-of", "csv=p=0"], f),
        )
    };
    let (ia, ib) = (info(&out_a), info(&out_b));
    eprintln!("script : {ia:?}\nnative : {ib:?}");
    assert_eq!(
        (&ia.0, &ia.1, &ia.2),
        (&ib.0, &ib.1, &ib.2),
        "stream layout or frame count differs"
    );
    let (da, db): (f64, f64) = (ia.3.parse().unwrap(), ib.3.parse().unwrap());
    assert!((da - db).abs() < 0.05, "durations differ: {da} vs {db}");

    let (va, vb) = (stream_md5(&out_a, true), stream_md5(&out_b, true));
    let (aa, ab) = (stream_md5(&out_a, false), stream_md5(&out_b, false));
    eprintln!(
        "video frames identical: {}   audio identical: {}",
        va == vb,
        aa == ab
    );
    if va != vb {
        let p = psnr(&out_a, &out_b);
        eprintln!("video differs; {p}");
        assert!(!p.contains("average:inf") || va == vb);
        // Not bit-identical (the GPU may not be deterministic): it must still be visually the same.
        let avg: f64 = p
            .split("average:")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        assert!(avg >= 45.0, "the two outputs differ visibly: {p}");
    }
    let _ = fs::remove_dir_all(&root);
}
