//! The real `vhs-gui upscale-worker` process against stub ffmpeg, ffprobe and upscaler programs:
//! the lock file, the process group, pause and resume with real signals, cancel, a second worker
//! being refused, and recovery after a worker is killed outright. (The pipeline's own behaviour is
//! covered by the unit tests in `src/upscale/engine.rs`.)

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

const STUB: &str = include_str!("support/stub.py");

/// The same shared launcher the unit tests use (see `src/upscale/testkit.rs`): written once, never
/// rewritten in place, so executing it cannot hit "Text file busy".
fn launcher() -> &'static Path {
    static L: OnceLock<PathBuf> = OnceLock::new();
    L.get_or_init(|| {
        let dir = std::env::temp_dir().join("vhs-gui-test-stubs");
        fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("stub.py");
        let launcher = dir.join("launcher");
        let text = format!(
            "#!/bin/sh\nexec python3 '{}' \"$(basename \"$0\")\" \"$(dirname \"$0\")/../state\" \"$@\"\n",
            stub.display()
        );
        for (path, content, mode) in [(&stub, STUB, 0o644), (&launcher, text.as_str(), 0o755)] {
            if fs::read_to_string(path).ok().as_deref() != Some(content) {
                let tmp = dir.join(format!(".tmp-{}", std::process::id()));
                fs::write(&tmp, content).unwrap();
                fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)).unwrap();
                fs::rename(&tmp, path).unwrap();
            }
        }
        launcher
    })
}

struct Rig {
    root: PathBuf,
    state: PathBuf,
    job: PathBuf,
    work_dir: PathBuf,
    output: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Rig {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("vhs-gui-worker-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["state", "bin", "models", "out", "work"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for tool in ["ffmpeg", "ffprobe", "realesrgan-ncnn-vulkan"] {
            std::os::unix::fs::symlink(launcher(), root.join("bin").join(tool)).unwrap();
        }
        fs::write(
            root.join("state/config.json"),
            "{\"duration\": 70.0, \"fps\": \"30000/1001\", \"w\": 720, \"h\": 480, \"audio\": true, \
             \"frames\": 4, \"upscaler_short_by\": 0, \"rebuild_short_by\": 0, \"dur_override_once\": {}}",
        )
        .unwrap();
        let input = root.join("Tape.mkv");
        fs::write(&input, b"source").unwrap();
        let output = root.join("out/Tape.upscale.mkv");
        let job = root.join("job.toml");
        fs::write(
            &job,
            format!(
                "variant = \"standard\"\ninput = \"{}\"\noutput = \"{}\"\nwork_root = \"{}\"\n\
                 segment_secs = 30\ncrf = 21\npreset = \"veryfast\"\nbackend = \"vulkan\"\n\
                 model = \"realesrgan-x4plus\"\nmodels_dir = \"{}\"\ninternal_scale = 4\n\
                 final_scale = 2\nthreads = \"3:3:3\"\nvk_device = 0\njpeg_quality = 2\n\
                 crush = \"none\"\nbrightness = \"\"\ntarget_dar = \"4:3\"\nallow_mixed = false\n\
                 skip_sync_fix = true\n",
                input.display(),
                output.display(),
                root.join("work").display(),
                root.join("models").display()
            ),
        )
        .unwrap();
        Self {
            state: root.join("state"),
            work_dir: root.join("work/Tape"),
            output,
            job,
            root,
        }
    }

    fn hold(&self) {
        fs::write(self.state.join("hold"), b"").unwrap();
    }

    fn release(&self) {
        let _ = fs::remove_file(self.state.join("hold"));
    }

    /// Start the worker the way the GUI does: in its own process group, output to a log.
    fn spawn(&self, name: &str) -> Child {
        let log = fs::File::create(self.root.join(format!("{name}.log"))).unwrap();
        Command::new(env!("CARGO_BIN_EXE_vhs-gui"))
            .arg("upscale-worker")
            .arg(&self.job)
            .env("VHS_GUI_FFMPEG", self.root.join("bin/ffmpeg"))
            .env("VHS_GUI_FFPROBE", self.root.join("bin/ffprobe"))
            .env(
                "VHS_GUI_REALESRGAN_NCNN",
                self.root.join("bin/realesrgan-ncnn-vulkan"),
            )
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(format!("{name}.log"))).unwrap_or_default()
    }

    fn wait_for(&self, what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_exit(&self, child: &mut Child) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(st) = child.try_wait().unwrap() {
                use std::os::unix::process::ExitStatusExt;
                return st.code().unwrap_or_else(|| -st.signal().unwrap_or(0));
            }
            assert!(Instant::now() < deadline, "the worker did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn running_upscale(&self) -> bool {
        self.state.join("upscaler_started").exists()
    }
}

fn state_of(pid: u32) -> char {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit_once(')')
                .and_then(|(_, r)| r.trim().chars().next())
        })
        .unwrap_or('?')
}

fn segments(rig: &Rig) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(rig.work_dir.join("segments"))
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn a_job_runs_to_the_end_and_removes_its_lock() {
    let rig = Rig::new("done");
    let mut w = rig.spawn("w");
    assert_eq!(rig.wait_exit(&mut w), 0, "{}", rig.log("w"));
    assert!(rig.output.is_file());
    assert_eq!(
        segments(&rig),
        ["seg_000.mp4", "seg_001.mp4", "seg_002.mp4"]
    );
    assert!(
        !rig.work_dir.join("upscale.pgid").exists(),
        "the lock must go at exit"
    );
    assert!(rig.log("w").contains("All done."));
}

#[test]
fn the_lock_names_the_workers_own_process_group_and_pause_resume_work() {
    let rig = Rig::new("pause");
    rig.hold();
    let mut w = rig.spawn("w");
    rig.wait_for("the upscaler to start", || rig.running_upscale());

    // `upscale.pgid` holds the worker's group, which the worker leads (the GUI relies on both).
    let lock = rig.work_dir.join("upscale.pgid");
    let pgid: i32 = fs::read_to_string(&lock).unwrap().trim().parse().unwrap();
    assert_eq!(pgid as u32, w.id());
    let cmdline = fs::read(format!("/proc/{}/cmdline", w.id())).unwrap();
    assert!(cmdline.split(|&b| b == 0).any(|a| a == b"upscale-worker"));

    // Pause the whole group, as the GUI's Pause does: the worker stops...
    killpg(Pid::from_raw(pgid), Signal::SIGSTOP).unwrap();
    rig.wait_for("the worker to stop", || state_of(w.id()) == 'T');
    // ...and carries on after SIGCONT.
    killpg(Pid::from_raw(pgid), Signal::SIGCONT).unwrap();
    rig.release();
    assert_eq!(rig.wait_exit(&mut w), 0, "{}", rig.log("w"));
    assert!(rig.output.is_file());
}

#[test]
fn sigint_cancels_cleanly_without_a_checkpoint_for_the_segment_in_progress() {
    let rig = Rig::new("cancel");
    rig.hold();
    let mut w = rig.spawn("w");
    rig.wait_for("the upscaler to start", || rig.running_upscale());

    // The GUI's Cancel (and Stop after segment) is SIGINT to the group.
    killpg(Pid::from_raw(w.id() as i32), Signal::SIGINT).unwrap();
    assert_eq!(rig.wait_exit(&mut w), 130, "{}", rig.log("w"));
    assert!(segments(&rig).is_empty());
    assert!(!rig.work_dir.join("seg_part").exists());
    assert!(!rig.work_dir.join("upscale.pgid").exists());
    assert!(!rig.output.exists());
}

#[test]
fn a_second_worker_on_the_same_job_is_refused() {
    let rig = Rig::new("twice");
    rig.hold();
    let mut first = rig.spawn("first");
    rig.wait_for("the first upscaler to start", || rig.running_upscale());

    let mut second = rig.spawn("second");
    assert_eq!(rig.wait_exit(&mut second), 1);
    assert!(
        rig.log("second").contains("already running"),
        "{}",
        rig.log("second")
    );
    // The first one is unharmed.
    assert!(first.try_wait().unwrap().is_none());
    rig.release();
    assert_eq!(rig.wait_exit(&mut first), 0, "{}", rig.log("first"));
}

#[test]
fn a_killed_worker_leaves_a_stale_lock_and_the_next_run_recovers() {
    let rig = Rig::new("killed");
    rig.hold();
    let mut w = rig.spawn("w");
    rig.wait_for("the upscaler to start", || rig.running_upscale());

    // A crash or SIGKILL: no cleanup runs, so the lock file is left behind.
    killpg(Pid::from_raw(w.id() as i32), Signal::SIGKILL).unwrap();
    let _ = w.wait();
    assert!(rig.work_dir.join("upscale.pgid").exists());

    rig.release();
    let mut again = rig.spawn("again");
    assert_eq!(rig.wait_exit(&mut again), 0, "{}", rig.log("again"));
    assert_eq!(segments(&rig).len(), 3);
    assert!(rig.output.is_file());
}

#[test]
fn bad_usage_is_reported() {
    let bin = env!("CARGO_BIN_EXE_vhs-gui");
    let none = Command::new(bin).arg("upscale-worker").output().unwrap();
    assert_eq!(none.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&none.stderr).contains("usage"));

    let missing = Command::new(bin)
        .args(["upscale-worker", "/nonexistent/job.toml"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot read the job file"));
}
