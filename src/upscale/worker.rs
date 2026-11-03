//! The detached upscale worker: `vhs-gui upscale-worker JOB.toml`.
//!
//! The GUI starts it in its own process group and then forgets about it, exactly as it did with
//! the bash script, so a job outlives the GUI. The GUI finds a running worker again through
//! `WORK_DIR/upscale.pgid` (written here, removed on exit) and controls it with group signals:
//! SIGSTOP/SIGCONT pause and resume, SIGINT cancels (also "Stop after segment", which the GUI
//! sends between segments).
//!
//! Exit status: 0 done, 130 cancelled, 1 failed, 2 bad usage.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

use super::engine::{Binaries, Engine, Outcome};
use super::plan::JobSpec;

/// The command-line word that selects this mode; `main` checks for it before any GUI setup.
pub const SUBCOMMAND: &str = "upscale-worker";

/// Set by the signal handler; the engine checks it between and after every step.
static CANCEL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop_signal(_: nix::libc::c_int) {
    CANCEL.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    let action = SigAction::new(
        SigHandler::Handler(on_stop_signal),
        SaFlags::empty(),
        SigSet::empty(),
    );
    for sig in [Signal::SIGINT, Signal::SIGTERM] {
        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        let _ = unsafe { sigaction(sig, &action) };
    }
}

/// `upscale.pgid`, written for the life of the run. A worker killed outright leaves it behind;
/// `PipelineJob::check_lock` discards a stale one.
struct PgidFile(PathBuf);

impl PgidFile {
    fn create(path: PathBuf) -> std::io::Result<Self> {
        fs::write(&path, format!("{}\n", nix::unistd::getpgrp()))?;
        Ok(Self(path))
    }
}

impl Drop for PgidFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Entry point; `args` are the arguments after `upscale-worker`. Returns the exit status.
pub fn main(args: &[String]) -> i32 {
    let Some(job_path) = args.first() else {
        eprintln!("usage: vhs-gui {SUBCOMMAND} JOB.toml");
        return 2;
    };
    let spec: JobSpec = match fs::read_to_string(job_path)
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: cannot read the job file {job_path}: {e}");
            return 2;
        }
    };
    run(&spec)
}

fn run(spec: &JobSpec) -> i32 {
    install_signal_handlers();
    let layout = spec.layout();

    // Two workers on one work directory would corrupt each other's checkpoints.
    if let Some(pid) = crate::pipeline::PipelineJob::check_lock(&layout.work_dir) {
        eprintln!("Error: an upscale of this file is already running (process group {pid}).");
        return 1;
    }
    if let Err(e) = fs::create_dir_all(&layout.work_dir) {
        eprintln!("Error: cannot create {}: {e}", layout.work_dir.display());
        return 1;
    }
    let _lock = match PgidFile::create(layout.pgid_file.clone()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Error: cannot write {}: {e}", layout.pgid_file.display());
            return 1;
        }
    };

    let bins = match Binaries::resolve(spec) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };
    match Engine::new(spec, bins, &CANCEL).run() {
        Ok(Outcome::Done) => 0,
        Ok(Outcome::Cancelled) => {
            eprintln!("Cancelled.");
            130
        }
        Err(e) => {
            eprintln!("Error: {e:#}");
            1
        }
    }
}
