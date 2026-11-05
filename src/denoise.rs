//! Native port of `vhs_denoise.sh` + `denoise.sh`: the Denoise button, and the first half of
//! Denoise+QTGMC.
//!
//! The GUI ran the script with no arguments or environment, so this is the script's defaults:
//! `DENOISE_PRESET=light`, `NOISERED_ENABLE=0` (denoise.sh ships with noise reduction OFF, so the
//! "denoise" is a 20 Hz high-pass, a timestamp rebase and 48 kHz stereo), and `NORM_DB=-2`.
//! Three steps, as in the script:
//!   1. ffmpeg: extract the audio to a PCM WAV (shared with Audio Cleanup, `extract_full_args`)
//!   2. sox: `norm -2`
//!   3. ffmpeg: copy the video bit-exact and mux the processed PCM audio (`remux_args`)
//!
//! Output is `captures/stabilized/<stem>_<HH-MM-SS>_STABLE.mkv`, the time being the local clock at
//! launch, as `date +%H-%M-%S` gave in the script.
//!
//! Not ported: the `DENOISE_PRESET`/`NOISE_*`/`NR_AMOUNT`/`NORM_DB`/`FFMPEG_THREADS`/`REBASING_MODE`
//! environment overrides and the positional arguments (the GUI never used them), and the dormant
//! SoX `noisered` branch. Audio Cleanup is the GUI's noise-reduction tool.
//!
//! [`steps`] is pure so it can be compared with what the script actually runs (the ignored
//! `matches_the_bash_script` test).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::audio_cleanup::{
    checked_sox_step, extract_full_args, remux_args, sox_command, unique_work_dir,
};
use crate::config::Config;
use crate::pipeline::{PipelineJob, SeqStep};
use crate::tools::{self, Tool};

/// SoX `norm` target in dBFS (the script's default `NORM_DB`).
const NORM_DB: &str = "-2";

/// One external command of the denoise, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub label: &'static str,
    pub tool: Tool,
    pub args: Vec<OsString>,
}

/// The three commands for `input` -> `output`, using `work` for the intermediate WAVs.
pub fn steps(input: &Path, output: &Path, work: &Path) -> Vec<Step> {
    let full = work.join("full.wav");
    let norm = work.join("norm.wav");
    vec![
        Step {
            label: "Extracting audio...",
            tool: Tool::Ffmpeg,
            args: extract_full_args(input, &full),
        },
        Step {
            label: "Normalizing...",
            tool: Tool::Sox,
            args: vec![
                full.clone().into(),
                norm.clone().into(),
                "norm".into(),
                NORM_DB.into(),
            ],
        },
        Step {
            label: "Muxing...",
            tool: Tool::Ffmpeg,
            args: remux_args(input, &norm, output),
        },
    ]
}

/// `captures/stabilized/<stem>_<hms>_STABLE.mkv`; the stem drops a trailing `.mkv` only.
pub fn output_path_at(input: &Path, cfg: &Config, hms: &str) -> PathBuf {
    let name = input.file_name().and_then(|s| s.to_str()).unwrap_or("out");
    let stem = name.strip_suffix(".mkv").unwrap_or(name);
    cfg.stabilized_dir()
        .join(format!("{stem}_{hms}_STABLE.mkv"))
}

pub enum Launch {
    /// The denoise is running; the path is where its output will appear.
    Started(Box<PipelineJob>, PathBuf),
    /// An output with this exact name already exists and was left alone.
    Exists(PathBuf),
}

pub fn launch(input: &Path, cfg: &Config, label: String) -> anyhow::Result<Launch> {
    tools::require(Tool::Ffmpeg)?;
    tools::require(Tool::Sox)?;
    if !input.is_file() {
        anyhow::bail!("input not found: {}", input.display());
    }
    let output = output_path_at(input, cfg, &crate::clock::hms());
    if output.exists() {
        return Ok(Launch::Exists(output));
    }
    fs::create_dir_all(cfg.stabilized_dir())?;

    let work_dir = unique_work_dir("denoise", &output);
    fs::create_dir_all(&work_dir)?;
    let seq: Vec<(&'static str, SeqStep)> = steps(input, &output, &work_dir)
        .into_iter()
        .map(|s| match s.tool {
            Tool::Sox => {
                let mut cmd = sox_command(&work_dir);
                cmd.args(&s.args);
                // The only sox step is full.wav -> norm.wav, which keeps the length.
                let wavs = (PathBuf::from(&s.args[0]), PathBuf::from(&s.args[1]));
                (s.label, checked_sox_step(cmd, Some(wavs)))
            }
            tool => {
                let mut cmd = tools::command(tool);
                cmd.args(&s.args);
                (s.label, SeqStep::Cmd(cmd))
            }
        })
        .collect();

    let job = PipelineJob::start_native_sequence(
        label,
        input,
        seq,
        Some(work_dir.clone()),
        &cfg.log_dir(),
    )
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&work_dir);
    })?;
    Ok(Launch::Started(Box::new(job), output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

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

    fn nproc() -> String {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .to_string()
    }

    #[test]
    fn three_steps_in_the_scripts_order() {
        let s = steps(Path::new("/in.mkv"), Path::new("/out.mkv"), Path::new("/w"));
        let tools: Vec<Tool> = s.iter().map(|x| x.tool).collect();
        assert_eq!(tools, [Tool::Ffmpeg, Tool::Sox, Tool::Ffmpeg]);
        let labels: Vec<&str> = s.iter().map(|x| x.label).collect();
        assert_eq!(
            labels,
            ["Extracting audio...", "Normalizing...", "Muxing..."]
        );
    }

    #[test]
    fn normalize_step_is_sox_norm_minus_two() {
        let s = steps(Path::new("/in.mkv"), Path::new("/out.mkv"), Path::new("/w"));
        assert_eq!(
            strings(&s[1].args),
            ["/w/full.wav", "/w/norm.wav", "norm", "-2"]
        );
    }

    #[test]
    fn extract_and_mux_arguments_are_exactly_the_scripts() {
        let s = steps(Path::new("/in.mkv"), Path::new("/out.mkv"), Path::new("/w"));
        let extract = format!(
            "-hide_banner -nostdin -y -fflags +genpts -i /in.mkv -vn -map 0:a:0 \
             -af highpass=f=20,aresample=async=0:first_pts=0,asetpts=N/SR/TB \
             -ac 2 -ar 48000 -c:a pcm_s16le -threads {} /w/full.wav",
            nproc()
        );
        assert_eq!(strings(&s[0].args).join(" "), extract);
        let mux = "-hide_banner -nostdin -y -fflags +genpts -i /in.mkv -fflags +genpts -i /w/norm.wav \
                   -map 0:v:0 -map 1:a:0 -c:v copy -c:a pcm_s16le -avoid_negative_ts make_zero \
                   -shortest /out.mkv";
        assert_eq!(strings(&s[2].args).join(" "), mux);
    }

    #[test]
    fn output_name_is_stem_time_stable() {
        let cfg = Config::default();
        let dir = cfg.stabilized_dir();
        assert_eq!(
            output_path_at(
                Path::new("/a/VHS_ARCHIVAL_2026-11-03_12-45-11.mkv"),
                &cfg,
                "13-04-09"
            ),
            dir.join("VHS_ARCHIVAL_2026-11-03_12-45-11_13-04-09_STABLE.mkv")
        );
        assert_eq!(
            output_path_at(Path::new("/a/noext"), &cfg, "01-02-03"),
            dir.join("noext_01-02-03_STABLE.mkv")
        );
    }

    /// Runs the real `vhs_denoise.sh` (which calls `denoise.sh`) with stub ffmpeg and sox that
    /// record their calls and create their output file, and checks the Rust steps are identical.
    /// Needs the sibling vhs-cli checkout, so it is not run by default.
    #[test]
    #[ignore]
    fn matches_the_bash_script() {
        let script = Config::default().scripts_dir.join("vhs_denoise.sh");
        assert!(script.is_file(), "script missing: {}", script.display());

        let guard = TempDir::new("vhs-gui-denoise");
        let dir = guard.0.clone();
        let stub = |name: &str, body: &str| {
            let p = dir.join(name);
            fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let log = "$(dirname \"$0\")/calls.txt";
        let ffmpeg = stub(
            "ffmpeg",
            &format!(
                "{{ echo CALL ffmpeg; printf '%s\\n' \"$@\"; }} >> \"{log}\"\n\
                 for a in \"$@\"; do last=\"$a\"; done\n: > \"$last\""
            ),
        );
        let sox = stub(
            "sox",
            &format!("{{ echo CALL sox; printf '%s\\n' \"$@\"; }} >> \"{log}\"\n: > \"$2\""),
        );
        let input = dir.join("in.mkv");
        fs::write(&input, "").unwrap();
        let output = dir.join("out_STABLE.mkv");

        let status = Command::new("bash")
            .arg(&script)
            .arg(&input)
            .arg(&output)
            .env("FFMPEG_BIN", &ffmpeg)
            .env("SOX_BIN", &sox)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "script failed");

        // The script's mktemp dir differs from ours; compare WAV paths by file name only. One
        // deliberate difference: with noise reduction off the script runs `cp -f full.wav
        // clean.wav` and normalizes `clean.wav`; we normalize `full.wav` directly and skip the
        // copy (an extra full-length WAV, about 1.3 GB for a 2 h tape). The audio is identical.
        let norm = |a: &str| {
            if a.ends_with(".wav") {
                let name = Path::new(a).file_name().unwrap().to_string_lossy();
                if name == "clean.wav" {
                    "full.wav".into()
                } else {
                    name.into_owned()
                }
            } else {
                a.to_string()
            }
        };
        let mut from_script: Vec<(String, Vec<String>)> = Vec::new();
        for line in fs::read_to_string(dir.join("calls.txt")).unwrap().lines() {
            if let Some(tool) = line.strip_prefix("CALL ") {
                from_script.push((tool.to_string(), Vec::new()));
            } else {
                from_script.last_mut().unwrap().1.push(norm(line));
            }
        }

        let ours: Vec<(String, Vec<String>)> = steps(&input, &output, Path::new("/w"))
            .iter()
            .map(|s| {
                (
                    s.tool.binary().to_string(),
                    strings(&s.args).iter().map(|a| norm(a)).collect(),
                )
            })
            .collect();
        assert_eq!(ours, from_script);
    }

    /// The real `launch` path with real ffmpeg and sox on a generated clip with a loud tone:
    /// the output keeps the video stream, has 48 kHz stereo PCM audio, and its peak is at the
    /// `sox norm -2` target. Writes only under a temp directory.
    #[test]
    #[ignore]
    fn end_to_end_smoke() {
        let guard = TempDir::new("vhs-gui-denoise-e2e");
        let dir = guard.0.clone();
        let cfg = Config {
            videos_dir: dir.clone(),
            ..Config::default()
        };

        let input = dir.join("clip.mkv");
        let made = tools::command(Tool::Ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=30:duration=3",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3,volume=0.3",
            ])
            .args([
                "-c:v",
                "ffv1",
                "-c:a",
                "pcm_s16le",
                "-ac",
                "1",
                "-ar",
                "44100",
                "-shortest",
            ])
            .arg(&input)
            .status()
            .unwrap();
        assert!(made.success());

        let Launch::Started(mut job, output) =
            launch(&input, &cfg, "smoke denoise".into()).unwrap()
        else {
            panic!("expected the denoise to start");
        };
        assert!(output.starts_with(dir.join("captures/stabilized")));
        assert!(output.to_string_lossy().ends_with("_STABLE.mkv"));
        for _ in 0..600 {
            job.poll();
            if job.done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(job.done, "denoise did not finish in 60 s");
        assert_eq!(job.exit_ok, Some(true));

        let probe = |sel: &str, entries: &str| {
            let out = tools::command(Tool::Ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    sel,
                    "-show_entries",
                    entries,
                ])
                .args(["-of", "csv=p=0"])
                .arg(&output)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert_eq!(
            probe("v:0", "stream=codec_name,width,height"),
            "ffv1,320,240"
        );
        assert_eq!(
            probe("a:0", "stream=codec_name,sample_rate,channels"),
            "pcm_s16le,48000,2"
        );

        // Peak level should sit at the norm target (-2 dBFS).
        let stats = tools::command(Tool::Ffmpeg)
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(&output)
            .args([
                "-vn",
                "-af",
                "astats=measure_overall=Peak_level:measure_perchannel=none",
                "-f",
                "null",
                "-",
            ])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&stats.stderr);
        let peak: f64 = text
            .lines()
            .rev()
            .find_map(|l| l.split("Peak level dB:").nth(1))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| panic!("no peak level in:\n{text}"));
        assert!(
            (peak + 2.0).abs() < 0.15,
            "peak {peak} dB, expected about -2"
        );

        // The scratch work directory is gone.
        let leftovers = fs::read_dir(cfg.stabilized_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!(".vhs-gui-denoise-{}-", std::process::id()))
            })
            .count();
        assert_eq!(leftovers, 0, "work dir not cleaned up");
    }
}
