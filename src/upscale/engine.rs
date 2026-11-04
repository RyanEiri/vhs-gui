//! The upscale pipeline, in the scripts' order (see `docs/upscale-engine-design.md`):
//! probe, fingerprint guard, per-segment extract / upscale / rebuild with checks, validation with
//! one auto-repair pass, concat, audio mux, sync fix.
//!
//! The external programs are injected ([`Binaries`]) so tests can run the whole engine against
//! stubs. Child processes inherit the worker's process group and stdio, so a group signal reaches
//! them and their output lands in the job log, as with the scripts.
//!
//! One deliberate difference from the scripts: a segment is encoded into `seg_part/` and renamed
//! into `segments/` only after its frame count checks out, so an encode killed halfway can never be
//! mistaken for a finished segment on resume.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, anyhow, bail};

use super::plan::{self, Backend, JobSpec, Layout, Variant};

/// The external programs a job runs.
#[derive(Clone, Debug)]
pub struct Binaries {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub upscaler: PathBuf,
}

impl Binaries {
    /// Resolve through the tool resolver (`VHS_GUI_*` overrides, bundled, `/usr/bin`, `~/bin`).
    pub fn resolve(spec: &JobSpec) -> anyhow::Result<Self> {
        use crate::tools::{self, Tool};
        Ok(Self {
            ffmpeg: tools::require(Tool::Ffmpeg)?,
            ffprobe: tools::require(Tool::Ffprobe)?,
            upscaler: tools::require(match spec.backend {
                Backend::Rocm => Tool::RealesrganRocm,
                Backend::Vulkan => Tool::RealesrganNcnn,
            })?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// The cancel flag was set (SIGINT to the worker, which "Cancel" and "Stop after segment" use).
    Cancelled,
}

/// Marker error for a cancelled run.
#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// What the segment loop ended with.
#[derive(Debug, PartialEq, Eq)]
enum LoopEnd {
    Finished,
    /// A segment produced no frames: the scripts stop the loop and go on to validation.
    NoFrames,
}

/// Values probed once and shared by the loop.
struct Ctx {
    duration: f64,
    duration_text: String,
    fps: String,
    size: (u32, u32),
    total: u64,
    count: u64,
    filter: String,
}

struct Validation {
    ok: bool,
    bad: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

pub struct Engine<'a> {
    spec: &'a JobSpec,
    bins: Binaries,
    layout: Layout,
    cancel: &'a AtomicBool,
    /// Silence child output (tests).
    quiet: bool,
}

impl<'a> Engine<'a> {
    pub fn new(spec: &'a JobSpec, bins: Binaries, cancel: &'a AtomicBool) -> Self {
        Self {
            layout: spec.layout(),
            spec,
            bins,
            cancel,
            quiet: false,
        }
    }

    #[cfg(test)]
    fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Run the job. A cancelled run removes its half-written segment and returns
    /// [`Outcome::Cancelled`]; any other failure is an error carrying the scripts' message.
    pub fn run(&self) -> anyhow::Result<Outcome> {
        match self.run_inner() {
            Ok(()) => Ok(Outcome::Done),
            Err(e) if e.downcast_ref::<Cancelled>().is_some() => {
                let _ = fs::remove_dir_all(&self.layout.part_dir);
                Ok(Outcome::Cancelled)
            }
            Err(e) => Err(e),
        }
    }

    // ---- running things --------------------------------------------------------------------

    fn command(&self, prog: &Path, args: &[OsString]) -> Command {
        let mut cmd = Command::new(prog);
        cmd.args(args).stdin(Stdio::null());
        if self.quiet {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
        cmd
    }

    /// Run a program to completion. A set cancel flag wins over the exit status (the group SIGINT
    /// that set it also killed the child).
    fn run_tool(&self, name: &str, prog: &Path, args: &[OsString]) -> anyhow::Result<()> {
        let mut cmd = self.command(prog, args);
        if prog == self.bins.upscaler
            && let Some(b) = self.spec.batch_size
        {
            cmd.env("BATCH_SIZE", b.to_string());
        }
        let status = cmd
            .status()
            .with_context(|| format!("could not start {name} ({})", prog.display()))?;
        if self.cancelled() {
            return Err(Cancelled.into());
        }
        if !status.success() {
            bail!("{name} failed ({status})");
        }
        Ok(())
    }

    fn ffmpeg(&self, args: &[OsString]) -> anyhow::Result<()> {
        self.run_tool("ffmpeg", &self.bins.ffmpeg, args)
    }

    /// ffprobe's trimmed stdout. `None` only when ffprobe could not be started.
    fn probe(&self, args: &[OsString]) -> Option<String> {
        let out = Command::new(&self.bins.ffprobe)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    // ---- the pipeline ----------------------------------------------------------------------

    fn run_inner(&self) -> anyhow::Result<()> {
        let spec = self.spec;
        let l = &self.layout;

        if !spec.input.is_file() {
            bail!("input file '{}' not found.", spec.input.display());
        }
        if spec.backend == Backend::Vulkan {
            if !plan::vulkan_supports(&spec.model) {
                bail!("{}", plan::vulkan_model_error(&spec.model));
            }
            if !spec.models_dir.is_dir() {
                bail!("MODELS_DIR '{}' not found.", spec.models_dir.display());
            }
        }
        for d in [&l.segments_dir, &l.frames_dir, &l.upscaled_dir] {
            fs::create_dir_all(d)?;
        }
        // A partial encode from an earlier run that was killed.
        let _ = fs::remove_dir_all(&l.part_dir);

        let ctx = self.probe_input()?;
        self.check_fingerprint(&ctx)?;
        self.print_header(&ctx);

        self.process_segments(&ctx, false)?;

        let mut v = self.validate(&ctx);
        if !v.ok {
            if v.bad.is_empty() {
                println!(">>> Auto-repair: duration drift detected, re-running segment loop...\n");
            } else {
                println!(
                    "\n>>> Auto-repair: removing {} bad segment(s) and regenerating...",
                    v.bad.len()
                );
                for f in &v.bad {
                    println!(
                        "  rm {}",
                        f.file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default()
                    );
                    let _ = fs::remove_file(f);
                }
                println!();
            }
            self.process_segments(&ctx, true)?;
            v = self.validate(&ctx);
            if !v.ok {
                bail!(
                    "segments still invalid after auto-repair. Aborting. Inspect: {}",
                    l.segments_dir.display()
                );
            }
        }

        self.assemble(&v.files)
    }

    fn probe_input(&self) -> anyhow::Result<Ctx> {
        let spec = self.spec;
        let input = &spec.input;
        let duration_text = self
            .probe(&plan::probe_duration_args(input))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("could not determine input duration."))?;
        let duration: f64 = duration_text
            .parse()
            .map_err(|_| anyhow!("could not determine input duration."))?;
        let fps = match self
            .probe(&plan::probe_stream_args("r_frame_rate", input))
            .filter(|s| !s.is_empty())
        {
            Some(f) => f,
            None => {
                eprintln!("Warning: could not determine r_frame_rate; defaulting to 30000/1001");
                "30000/1001".into()
            }
        };
        let dim = |entry: &str| -> anyhow::Result<u32> {
            self.probe(&plan::probe_stream_args(entry, input))
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow!("could not read the source {entry}."))
        };
        let (src_w, src_h) = (dim("width")?, dim("height")?);

        plan::validate_scales(spec.internal_scale, spec.final_scale).map_err(|e| anyhow!(e))?;
        let size = plan::output_size(src_w, src_h, spec.final_scale, &spec.target_dar);
        let filter = plan::pre_filter(spec.variant, &spec.crush, &spec.brightness)
            .map_err(|e| anyhow!(e))?;
        let total = plan::total_seconds(duration);
        Ok(Ctx {
            duration,
            duration_text,
            fps,
            size,
            total,
            count: plan::segment_count(total, spec.segment_secs),
            filter,
        })
    }

    /// Refuse to mix settings: if segments exist and `run_config.txt` differs from this job's
    /// fingerprint, stop (unless `allow_mixed`). Then record this job's fingerprint.
    fn check_fingerprint(&self, ctx: &Ctx) -> anyhow::Result<()> {
        let l = &self.layout;
        let payload = plan::fingerprint(self.spec, &ctx.fps, &ctx.filter);
        let has_segments = fs::read_dir(&l.segments_dir)?
            .filter_map(Result::ok)
            .any(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.starts_with("seg_") && n.ends_with(".mp4")
            });
        if let Ok(old) = fs::read_to_string(&l.config_file)
            && has_segments
            && !self.spec.allow_mixed
            && old != payload
        {
            bail!(
                "existing segments found, but current configuration differs from the \
                 configuration used to generate them.\n\nWork dir : {}\nConfig   : {}\n\n\
                 To proceed safely remove the segments (rm -f '{}'/seg_*.mp4) or the work dir, \
                 or override with ALLOW_MIXED (not recommended).\n\nCurrent effective \
                 configuration:\n{payload}",
                l.work_dir.display(),
                l.config_file.display(),
                l.segments_dir.display()
            );
        }
        fs::write(&l.config_file, payload)?;
        Ok(())
    }

    fn print_header(&self, ctx: &Ctx) {
        let s = self.spec;
        let l = &self.layout;
        let label = match s.variant {
            Variant::Standard => "VHS upscale",
            Variant::Anime => "VHS animation upscale",
            Variant::Bw => "VHS B&W upscale",
        };
        println!(">>> {label} (chunked, resumable)");
        println!("Input file      : {}", s.input.display());
        println!("Output file     : {}", s.output.display());
        println!("Chunk length    : {}s", s.segment_secs);
        println!("CRF             : {}", s.crf);
        println!("x264 preset     : {}", s.preset);
        println!(
            "Model           : {} (internal {}x, final {}x)",
            s.model, s.internal_scale, s.final_scale
        );
        println!("Work dir        : {}", l.work_dir.display());
        println!("Duration        : ~{}s", ctx.total);
        println!("FPS             : {}", ctx.fps);
        println!("Output          : {}x{}", ctx.size.0, ctx.size.1);
        if !ctx.filter.is_empty() {
            println!("Pre-filter      : {}", ctx.filter);
        }
        println!();
    }

    fn count_frames(dir: &Path) -> usize {
        fs::read_dir(dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The per-segment loop; `repair` is the second pass after a failed validation.
    fn process_segments(&self, ctx: &Ctx, repair: bool) -> anyhow::Result<LoopEnd> {
        let l = &self.layout;
        for i in 0..ctx.count {
            if self.cancelled() {
                return Err(Cancelled.into());
            }
            let (start, len) = plan::segment_bounds(i, self.spec.segment_secs, ctx.total);
            let seg_out = l.segment_path(i);
            let tag = format!("[seg {i:03}]");

            if fs::metadata(&seg_out).is_ok_and(|m| m.len() > 0) {
                if !repair {
                    println!("{tag} exists - skipping ({})", seg_out.display());
                }
                continue;
            }
            if repair {
                println!("{tag} REPAIR: start={start}s len={len}s");
            } else {
                println!("{tag} start={start}s len={len}s");
            }

            for d in [&l.frames_dir, &l.upscaled_dir] {
                let _ = fs::remove_dir_all(d);
                fs::create_dir_all(d)?;
            }

            if self.spec.variant == Variant::Bw {
                println!(
                    "  -> Extracting grayscale frames (stored as RGB JPG for compatibility)..."
                );
            } else {
                println!("  -> Extracting frames (video only)...");
            }
            self.ffmpeg(&plan::extract_args(self.spec, l, start, len, &ctx.filter))?;

            if Self::count_frames(&l.frames_dir) == 0 {
                eprintln!("  -> No frames extracted for this segment; stopping.");
                return Ok(LoopEnd::NoFrames);
            }

            println!("  -> Real-ESRGAN upscaling...");
            self.run_tool(
                "the upscaler",
                &self.bins.upscaler,
                &plan::upscale_args(self.spec, l),
            )?;

            let (given, made) = (
                Self::count_frames(&l.frames_dir),
                Self::count_frames(&l.upscaled_dir),
            );
            if given != made {
                bail!(
                    "Real-ESRGAN produced {made} frame(s), expected {given}.\n       Input : {}\n       Output: {}",
                    l.frames_dir.display(),
                    l.upscaled_dir.display()
                );
            }

            println!(
                "  -> Rebuilding segment video at {}x{} (DAR-correct)...",
                ctx.size.0, ctx.size.1
            );
            fs::create_dir_all(&l.part_dir)?;
            let part = l.part_path(i);
            let _ = fs::remove_file(&part);
            self.ffmpeg(&plan::rebuild_args(self.spec, l, &ctx.fps, ctx.size, &part))?;

            let expected = Self::count_frames(&l.upscaled_dir);
            let actual: usize = self
                .probe(&plan::count_packets_args(&part))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if expected != actual {
                bail!(
                    "rebuilt segment has {actual} frame(s), expected {expected}.\n       Frames: {}\n       Output: {}",
                    l.upscaled_dir.display(),
                    part.display()
                );
            }
            // Only a verified segment becomes a checkpoint.
            fs::rename(&part, &seg_out)
                .with_context(|| format!("could not move {} into place", part.display()))?;
            // Remove the staging directory again (only succeeds when it is empty).
            let _ = fs::remove_dir(&l.part_dir);

            for d in [&l.frames_dir, &l.upscaled_dir] {
                let _ = fs::remove_dir_all(d);
            }
            println!();
        }
        Ok(LoopEnd::Finished)
    }

    fn segment_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.layout.segments_dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name().is_some_and(|n| {
                            let n = n.to_string_lossy();
                            n.starts_with("seg_") && n.ends_with(".mp4")
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Check every segment is readable and not suspiciously short (except the last), and that the
    /// summed duration is within the variant's drift tolerance of the source.
    fn validate(&self, ctx: &Ctx) -> Validation {
        println!(">>> Validating segments...");
        let files = self.segment_files();
        if files.is_empty() {
            eprintln!(
                "Error: no segment files found in {}.",
                self.layout.segments_dir.display()
            );
            return Validation {
                ok: false,
                bad: Vec::new(),
                files,
            };
        }
        let last = files.last().cloned();
        let mut bad = Vec::new();
        let mut total = 0.0;
        for f in &files {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dur: Option<f64> = self
                .probe(&plan::probe_duration_args(f))
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok());
            let Some(d) = dur else {
                eprintln!("  CORRUPT: {name} — ffprobe could not read file");
                bad.push(f.clone());
                continue;
            };
            total += d;
            if Some(f) != last.as_ref() && plan::is_short(d, self.spec.segment_secs) {
                eprintln!(
                    "  SHORT:   {name} — {d}s (expected ~{}s)",
                    self.spec.segment_secs
                );
                bad.push(f.clone());
            }
        }
        let drift = (ctx.duration - total).abs();
        let pct = plan::drift_pct(ctx.duration, total);
        println!("  Segment count   : {}", files.len());
        println!("  Total video dur : {total:.3}s");
        println!("  Source duration  : {}s", ctx.duration_text);
        println!("  Drift            : {drift:.1}s ({pct:.1}%)");

        if !bad.is_empty() {
            return Validation {
                ok: false,
                bad,
                files,
            };
        }
        if pct > self.spec.variant.max_drift_pct() {
            eprintln!(
                "  WARNING: total segment duration drifts {pct:.1}% from source ({drift:.1}s)."
            );
            return Validation {
                ok: false,
                bad,
                files,
            };
        }
        println!("  Validation passed.\n");
        Validation {
            ok: true,
            bad,
            files,
        }
    }

    /// Concatenate, mux the source audio, then correct any A/V drift the segment encodes added.
    fn assemble(&self, files: &[PathBuf]) -> anyhow::Result<()> {
        let spec = self.spec;
        let l = &self.layout;

        let list: String = files
            .iter()
            .map(|f| {
                format!(
                    "file 'segments/{}'\n",
                    f.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                )
            })
            .collect();
        fs::write(&l.concat_list, list)?;

        println!(
            ">>> Concatenating {} segment(s) into: {}",
            files.len(),
            l.concat_video.display()
        );
        self.ffmpeg(&plan::concat_args(l))?;

        println!(
            ">>> Muxing original audio into final upscaled video: {}",
            spec.output.display()
        );
        let has_audio = self
            .probe(&plan::probe_audio_args(&spec.input))
            .is_some_and(|s| !s.is_empty());
        if !has_audio {
            eprintln!("Warning: no audio stream detected in input; writing video-only output.");
        }
        self.ffmpeg(&plan::mux_args(spec, l, has_audio))?;

        // The scripts only ran the sync fix when the source had audio.
        if has_audio && !spec.skip_sync_fix {
            let stem = spec.output.with_extension("");
            let tmp = PathBuf::from(format!("{}._synctmp.mkv", stem.display()));
            println!(">>> Checking A/V sync drift in output...");
            match crate::fix_sync::correct(&spec.output, &tmp) {
                Ok(true) if tmp.is_file() => {
                    fs::rename(&tmp, &spec.output)?;
                    println!("    Output replaced with sync-corrected version.");
                }
                Ok(_) => {}
                Err(e) => eprintln!("Warning: sync fix failed; keeping original output. ({e})"),
            }
            let _ = fs::remove_file(&tmp);
        }

        println!("\nAll done.");
        println!("Final upscaled file: {}", spec.output.display());
        println!("Work dir (resume/inspection): {}", l.work_dir.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upscale::testkit::{Kit, StubCfg};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn run(kit: &Kit) -> anyhow::Result<Outcome> {
        let cancel = AtomicBool::new(false);
        Engine::new(&kit.spec, kit.bins.clone(), &cancel)
            .quiet()
            .run()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn kit(name: &str) -> Kit {
        Kit::new(name, Variant::Standard, &StubCfg::default())
    }

    #[test]
    fn a_full_run_produces_segments_config_and_output() {
        let kit = kit("up-full");
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
        let l = kit.spec.layout();

        assert_eq!(
            names(&l.segments_dir),
            ["seg_000.mp4", "seg_001.mp4", "seg_002.mp4"]
        );
        assert!(kit.spec.output.is_file());
        assert_eq!(
            fs::read_to_string(&l.config_file).unwrap(),
            plan::fingerprint(&kit.spec, "30000/1001", "hqdn3d=3:2:4:3")
        );
        // Temporary directories are gone; nothing was left in the staging directory.
        assert!(!l.frames_dir.exists() && !l.upscaled_dir.exists() && !l.part_dir.exists());
        assert_eq!(
            fs::read_to_string(&l.concat_list).unwrap(),
            "file 'segments/seg_000.mp4'\nfile 'segments/seg_001.mp4'\nfile 'segments/seg_002.mp4'\n"
        );

        // The last segment is shortened to the source: 70 s in 30 s chunks.
        let extracts: Vec<Vec<String>> = kit
            .calls_of("ffmpeg")
            .into_iter()
            .filter(|a| {
                a.first().map(String::as_str) == Some("-y")
                    && a.get(1).map(String::as_str) == Some("-ss")
            })
            .collect();
        assert_eq!(extracts.len(), 3);
        assert_eq!(&extracts[2][2..5], ["60", "-t", "10"]);
        assert_eq!(kit.calls_of("upscaler").len(), 3);
    }

    #[test]
    fn a_finished_segment_is_skipped_on_resume() {
        let kit = kit("up-resume");
        run(&kit).unwrap();
        let l = kit.spec.layout();
        fs::remove_file(l.segment_path(2)).unwrap();
        let before = kit.calls_of("upscaler").len();
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
        assert_eq!(
            kit.calls_of("upscaler").len() - before,
            1,
            "only the missing segment is redone"
        );
        assert!(l.segment_path(2).is_file());
    }

    #[test]
    fn a_changed_configuration_refuses_to_mix_segments() {
        let mut kit = kit("up-mixed");
        run(&kit).unwrap();
        kit.spec.crf = 18;
        let err = run(&kit).unwrap_err().to_string();
        assert!(err.contains("configuration differs"), "{err}");
        assert!(
            err.contains("CRF=18"),
            "the current configuration is shown: {err}"
        );

        kit.spec.allow_mixed = true;
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
    }

    #[test]
    fn a_changed_configuration_is_fine_when_no_segments_exist_yet() {
        let mut kit = kit("up-fresh");
        run(&kit).unwrap();
        let l = kit.spec.layout();
        for i in 0..3 {
            fs::remove_file(l.segment_path(i)).unwrap();
        }
        kit.spec.crf = 18;
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
    }

    #[test]
    fn too_few_upscaled_frames_fail_loudly_and_leave_no_segment() {
        let kit = kit("up-short-up");
        kit.set(&StubCfg {
            upscaler_short_by: 1,
            ..StubCfg::default()
        });
        let err = run(&kit).unwrap_err().to_string();
        assert!(err.contains("produced 3 frame(s), expected 4"), "{err}");
        assert!(names(&kit.spec.layout().segments_dir).is_empty());
    }

    #[test]
    fn a_rebuilt_segment_with_missing_frames_never_becomes_a_checkpoint() {
        let kit = kit("up-short-rebuild");
        kit.set(&StubCfg {
            rebuild_short_by: 1,
            ..StubCfg::default()
        });
        let err = run(&kit).unwrap_err().to_string();
        assert!(
            err.contains("rebuilt segment has 3 frame(s), expected 4"),
            "{err}"
        );
        let l = kit.spec.layout();
        assert!(
            names(&l.segments_dir).is_empty(),
            "a bad encode must not be a checkpoint"
        );

        // The next run clears the leftover partial file and succeeds.
        kit.set(&StubCfg::default());
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
        assert!(!l.part_dir.exists());
    }

    #[test]
    fn a_stale_partial_encode_from_a_killed_run_is_discarded() {
        let kit = kit("up-stale-part");
        let l = kit.spec.layout();
        fs::create_dir_all(&l.part_dir).unwrap();
        fs::write(l.part_path(0), b"half an encode").unwrap();
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
        assert!(!l.part_dir.exists());
        assert!(names(&l.segments_dir).iter().all(|n| n.starts_with("seg_")));
    }

    #[test]
    fn cancelling_mid_segment_stops_without_a_checkpoint_for_it() {
        let kit = Arc::new(kit("up-cancel"));
        fs::write(kit.state.join("hold"), b"").unwrap(); // the upscaler waits until this goes
        let cancel = Arc::new(AtomicBool::new(false));

        let (k, c) = (Arc::clone(&kit), Arc::clone(&cancel));
        let worker = thread::spawn(move || {
            Engine::new(&k.spec, k.bins.clone(), &c)
                .quiet()
                .run()
                .unwrap()
        });

        let started = kit.state.join("upscaler_started");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !started.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(started.exists(), "the upscaler never started");
        cancel.store(true, Ordering::SeqCst); // what the group SIGINT does
        fs::remove_file(kit.state.join("hold")).unwrap(); // and the child dies

        assert_eq!(worker.join().unwrap(), Outcome::Cancelled);
        let l = kit.spec.layout();
        assert!(names(&l.segments_dir).is_empty());
        assert!(!l.part_dir.exists());
        assert!(!kit.spec.output.exists());
    }

    #[test]
    fn a_short_segment_is_repaired_once() {
        let kit = kit("up-repair");
        // The first encode of segment 1 reports 3 s instead of 30 s: suspiciously short.
        kit.set(&StubCfg {
            dur_override_once: vec![("seg_001.mp4", 3.0)],
            ..StubCfg::default()
        });
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
        assert_eq!(
            kit.calls_of("upscaler").len(),
            4,
            "segment 1 is regenerated"
        );
        assert!(kit.spec.output.is_file());
    }

    #[test]
    fn drift_that_a_repair_cannot_fix_aborts() {
        let kit = kit("up-drift");
        // The last segment comes out 5 s instead of 10 s: not "short" (it is the last one) but the
        // total is 7.1% off the source, over the 2% limit. A repair pass changes nothing.
        kit.set(&StubCfg {
            dur_override_once: vec![("seg_002.mp4", 5.0)],
            ..StubCfg::default()
        });
        let err = run(&kit).unwrap_err().to_string();
        assert!(err.contains("still invalid after auto-repair"), "{err}");
        assert!(!kit.spec.output.exists());
    }

    #[test]
    fn anime_tolerates_more_drift() {
        let kit = Kit::new("up-anime-drift", Variant::Anime, &StubCfg::default());
        kit.set(&StubCfg {
            dur_override_once: vec![("seg_002.mp4", 5.0)], // 7.1% drift: fine for anime (8%)
            ..StubCfg::default()
        });
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
    }

    #[test]
    fn a_source_without_audio_is_copied_video_only() {
        let kit = kit("up-noaudio");
        kit.set(&StubCfg {
            audio: false,
            ..StubCfg::default()
        });
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
        let mux = kit.calls_of("ffmpeg").pop().unwrap();
        assert!(
            mux.contains(&"-c".to_string()) && !mux.contains(&"aac".to_string()),
            "{mux:?}"
        );
    }

    #[test]
    fn input_and_model_problems_are_reported_up_front() {
        let mut kit = kit("up-preflight");
        kit.spec.input = kit.dir.0.join("missing.mkv");
        assert!(run(&kit).unwrap_err().to_string().contains("not found"));

        let mut kit = Kit::new("up-model", Variant::Standard, &StubCfg::default());
        kit.spec.model = "2x_VHS-Film".into();
        assert!(
            run(&kit)
                .unwrap_err()
                .to_string()
                .contains("not supported by the vulkan backend")
        );

        let mut kit = Kit::new("up-models-dir", Variant::Standard, &StubCfg::default());
        kit.spec.models_dir = kit.dir.0.join("no-such-dir");
        assert!(run(&kit).unwrap_err().to_string().contains("MODELS_DIR"));
        // ROCm ignores the models directory, so a missing one is not an error there.
        kit.spec.backend = Backend::Rocm;
        kit.spec.model = "2x_VHS-Film".into();
        assert_eq!(run(&kit).unwrap(), Outcome::Done);
    }

    #[test]
    fn rocm_batch_size_reaches_the_upscaler() {
        let mut kit = kit("up-batch");
        kit.spec.backend = Backend::Rocm;
        kit.spec.batch_size = Some(2);
        run(&kit).unwrap();
        assert_eq!(
            fs::read_to_string(kit.state.join("upscaler_started")).unwrap(),
            "2"
        );
    }

    #[test]
    fn bad_scale_math_is_rejected() {
        let mut kit = kit("up-scale");
        kit.spec.final_scale = 3;
        assert!(
            run(&kit)
                .unwrap_err()
                .to_string()
                .contains("evenly divisible")
        );
    }
}

#[cfg(test)]
mod golden {
    //! The engine against the real scripts: the same stubs, the same inputs, and the same calls and
    //! `run_config.txt` are required. Needs the sibling vhs-cli checkout, so it is ignored by
    //! default (`cargo test -- --ignored`).
    use super::*;
    use crate::config::Config;
    use crate::upscale::testkit::{Kit, StubCfg};
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// The script's tool calls and its `run_config.txt`, run with only stubs reachable.
    fn run_script(kit: &Kit, script_name: &str) -> (Vec<(String, Vec<String>)>, String) {
        let scripts = Config::default().scripts_dir;
        let original = fs::read_to_string(scripts.join(script_name)).unwrap();
        // Only the two hard-coded binary paths are replaced; the logic is the real script's.
        let patched = original
            .replace(
                "FFMPEG=\"/usr/bin/ffmpeg\"",
                &format!("FFMPEG=\"{}\"", kit.bins.ffmpeg.display()),
            )
            .replace(
                "FFPROBE=\"/usr/bin/ffprobe\"",
                &format!("FFPROBE=\"{}\"", kit.bins.ffprobe.display()),
            );
        assert_ne!(
            patched, original,
            "the binary paths were not found in {script_name}"
        );
        assert!(!patched.contains("FFMPEG=\"/usr/bin/ffmpeg\""));
        // No vhs_fix_sync.sh beside the copy, so the script skips its sync-fix step (the engine
        // skips it too in this test).
        let copy = kit.dir.0.join(script_name);
        fs::write(&copy, patched).unwrap();

        let spec = &kit.spec;
        let mut cmd = Command::new("bash");
        cmd.arg(&copy)
            .arg(&spec.input)
            .arg(&spec.output)
            .arg(spec.segment_secs.to_string())
            .arg(spec.crf.to_string())
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", kit.dir.0.join("bin").display()),
            )
            .env("HOME", &kit.dir.0)
            .env("WORK_ROOT", &spec.work_root)
            .env("MODELS_DIR", &spec.models_dir)
            .env("MODEL", &spec.model)
            .env("UPSCALE_BACKEND", spec.backend_name())
            .env("INTERNAL_SCALE", spec.internal_scale.to_string())
            .env("FINAL_SCALE", spec.final_scale.to_string())
            .env("CRUSH", &spec.crush)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if !spec.brightness.is_empty() {
            cmd.env("BRIGHTNESS", &spec.brightness);
        }
        use std::os::unix::process::CommandExt as _;
        let mut child = cmd.process_group(0).spawn().unwrap();
        let started = Instant::now();
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if started.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
                panic!("{script_name} did not finish in 60 s");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(status.success(), "{script_name} failed: {status}");
        assert!(!kit.calls().is_empty(), "the stubs were never called");
        let config = fs::read_to_string(spec.layout().config_file).unwrap();
        (kit.calls(), config)
    }

    /// Wipe what the script left so the engine starts from nothing, and the call log with it.
    fn reset(kit: &Kit) {
        let _ = fs::remove_dir_all(&kit.spec.work_root);
        let _ = fs::remove_file(&kit.spec.output);
        let _ = fs::remove_file(kit.state.join("calls.txt"));
        let _ = fs::remove_dir_all(kit.state.join("info"));
        let _ = fs::remove_file(kit.state.join("cur_len"));
    }

    /// The scripts have no staging directory: the engine encodes into `seg_part/` and renames. That
    /// is the one deliberate difference, so it is normalized away before comparing.
    fn normalize(calls: Vec<(String, Vec<String>)>) -> Vec<(String, Vec<String>)> {
        calls
            .into_iter()
            .map(|(t, a)| {
                let t = if t.starts_with("realesrgan") {
                    "upscaler".to_string()
                } else {
                    t
                };
                (
                    t,
                    a.into_iter()
                        .map(|x| x.replace("/seg_part/", "/segments/"))
                        .collect(),
                )
            })
            .collect()
    }

    fn compare(
        name: &str,
        variant: Variant,
        script: &str,
        cfg: StubCfg,
        tweak: impl Fn(&mut JobSpec),
    ) {
        let mut kit = Kit::new(name, variant, &cfg);
        tweak(&mut kit.spec);
        let (script_calls, script_config) = run_script(&kit, script);
        reset(&kit);

        let cancel = AtomicBool::new(false);
        let outcome = Engine::new(&kit.spec, kit.bins.clone(), &cancel)
            .quiet()
            .run()
            .unwrap();
        assert_eq!(outcome, Outcome::Done);
        let engine_config = fs::read_to_string(kit.spec.layout().config_file).unwrap();
        let engine_calls = normalize(kit.calls());

        assert_eq!(
            engine_config, script_config,
            "{name}: run_config.txt differs"
        );
        assert_eq!(
            engine_calls.len(),
            script_calls.len(),
            "{name}: different number of calls"
        );
        for (i, (e, s)) in engine_calls.iter().zip(script_calls.iter()).enumerate() {
            assert_eq!(e, s, "{name}: call {i} differs");
        }
    }

    #[test]
    #[ignore]
    fn the_standard_engine_matches_vhs_upscale_sh() {
        let base = StubCfg::default(); // 70 s: three segments, the last one 10 s
        compare(
            "gold-std",
            Variant::Standard,
            "vhs_upscale.sh",
            base.clone(),
            |_| {},
        );
        compare(
            "gold-std-short",
            Variant::Standard,
            "vhs_upscale.sh",
            StubCfg {
                duration: 12.0,
                ..base.clone()
            },
            |_| {},
        );
        compare(
            "gold-std-noaudio",
            Variant::Standard,
            "vhs_upscale.sh",
            StubCfg {
                audio: false,
                ..base.clone()
            },
            |_| {},
        );
        compare(
            "gold-std-rocm",
            Variant::Standard,
            "vhs_upscale.sh",
            base.clone(),
            |s| {
                s.backend = Backend::Rocm;
                s.model = "2x_VHS-Film".into();
                s.internal_scale = 2;
                s.final_scale = 1;
                s.crush = "medium".into();
                s.segment_secs = 20;
                s.crf = 18;
            },
        );
        compare(
            "gold-std-bright",
            Variant::Standard,
            "vhs_upscale.sh",
            base.clone(),
            |s| {
                s.crush = "heavy".into();
                s.brightness = "low".into();
            },
        );
        compare(
            "gold-std-pal",
            Variant::Standard,
            "vhs_upscale.sh",
            StubCfg {
                fps: "25/1",
                w: 720,
                h: 576,
                ..base
            },
            |_| {},
        );
    }

    #[test]
    #[ignore]
    fn the_anime_engine_matches_vhs_upscale_anime_sh() {
        let base = StubCfg::default();
        compare(
            "gold-anime",
            Variant::Anime,
            "vhs_upscale_anime.sh",
            base.clone(),
            |s| s.model = Variant::Anime.default_model().into(),
        );
        compare(
            "gold-anime-short",
            Variant::Anime,
            "vhs_upscale_anime.sh",
            StubCfg {
                duration: 12.0,
                ..base.clone()
            },
            |s| s.model = Variant::Anime.default_model().into(),
        );
        compare(
            "gold-anime-rocm",
            Variant::Anime,
            "vhs_upscale_anime.sh",
            base,
            |s| {
                s.backend = Backend::Rocm;
                s.model = Variant::Anime.default_model().into();
                s.crush = "medium".into();
                s.segment_secs = 20;
            },
        );
    }

    #[test]
    #[ignore]
    fn the_bw_engine_matches_vhs_upscale_bw_sh() {
        let base = StubCfg::default();
        compare(
            "gold-bw",
            Variant::Bw,
            "vhs_upscale_bw.sh",
            base.clone(),
            |_| {},
        );
        compare(
            "gold-bw-noaudio",
            Variant::Bw,
            "vhs_upscale_bw.sh",
            StubCfg {
                audio: false,
                ..base.clone()
            },
            |_| {},
        );
        compare(
            "gold-bw-rocm",
            Variant::Bw,
            "vhs_upscale_bw.sh",
            base,
            |s| {
                s.backend = Backend::Rocm;
                s.crush = "medium".into();
                s.brightness = "low".into();
            },
        );
    }
}
