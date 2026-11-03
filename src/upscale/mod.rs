//! The native upscale engine (formerly `vhs_upscale.sh`, `vhs_upscale_anime.sh` and
//! `vhs_upscale_bw.sh`). See `docs/upscale-engine-design.md`.
//!
//! * [`plan`]: pure functions: segment math, output size, filter chains, the resume fingerprint and
//!   the exact command arguments. Everything the golden tests compare with the scripts.
//! * [`engine`]: the segment loop, checks, validation, concat, mux and sync fix.
//! * [`worker`]: the detached `vhs-gui upscale-worker` process that runs the engine, so a job
//!   outlives the GUI exactly as the scripts' jobs did.

pub mod engine;
pub mod plan;
pub mod worker;

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::pipeline::PipelineJob;

/// Start a detached worker for `spec`: write its job file into the work directory and spawn
/// `vhs-gui upscale-worker` in its own process group (as `start_native_single` does), so the job
/// outlives the GUI. `/proc/self/exe` rather than `current_exe()` so it still works if the binary
/// on disk has been replaced since this GUI started.
pub fn start_worker(
    spec: &plan::JobSpec,
    label: String,
    log_dir: &Path,
) -> anyhow::Result<PipelineJob> {
    let layout = spec.layout();
    fs::create_dir_all(&layout.work_dir)?;
    let job_file = layout.work_dir.join("job.toml");
    fs::write(&job_file, toml::to_string(spec)?)?;
    let mut cmd = Command::new("/proc/self/exe");
    cmd.arg(worker::SUBCOMMAND).arg(&job_file);
    PipelineJob::start_native_single(label, &spec.input, cmd, log_dir)
}

#[cfg(test)]
mod testkit;
