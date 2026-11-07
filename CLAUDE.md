# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

A native Rust desktop application (eframe/egui) for VHS digitization: live capture
monitoring, a file library with per-category pipeline actions, embedded mpv
playback, and AI upscaling with resumable segment-checkpoint tracking. It's one of
three sibling projects forked from a former monorepo:

- **`vhs-gui`** (this repo) — the GUI, self-contained.
- **[`vhs-cli`](https://github.com/RyanEiri/vhs-cli)** — the bash pipeline scripts,
  for terminal/standalone use. Independent of this repo — no shared code. A handful
  of not-yet-natively-ported operations in this GUI still shell out to vhs-cli
  scripts as a transitional measure (see "Transitional bash dependency" below); this
  is being phased out.
- **[`plex-reencoder`](https://github.com/RyanEiri/plex-reencoder)** — unrelated
  Plex library tooling, no relationship to this repo.

All three expect to live as siblings under `~/Videos/` (`~/Videos/vhs-gui/`,
`~/Videos/vhs-cli/`, `~/Videos/plex-reencoder/`), alongside data directories
(`captures/`, `logs/`, `vhs_upscale_work/`) that belong to none of them — `src/
config.rs` references them by absolute path.

**Goal: self-contained.** New pipeline-operation work in this repo should be native
Rust spawning the underlying tool directly (ffmpeg, `vspipe`, Real-ESRGAN) — never a
new dependency on a vhs-cli script. Do not add new `Command::new("bash").arg(script)`
call sites.

## Build & Launch

```bash
cd vhs-gui && cargo build            # debug
DISPLAY=:0 ./target/debug/vhs-gui
```

## Architecture

- **`src/app.rs`** — shell: owns `Config`, `MpvView`, `MonitorPanel`, `UpscalePanel`,
  `ViewMode`, debounced settings persistence. Icon-only left rail switches between
  Monitor and Upscale views; each has its own collapsible settings side panel.
- **`src/pipeline.rs`** — `PipelineJob`: spawns a job (currently `bash <script>
  <input>`, `.process_group(0)` so `killpg()` never reaches vhs-gui itself), tails
  its log for ffmpeg `frame=`/`time=` progress (**time-based progress**, not
  frame-based — robust to fps changes from QTGMC/VDecimate), and exposes
  pause/resume/cancel via `killpg(SIGSTOP/SIGCONT/SIGINT)`. Upscale jobs additionally
  track segment-checkpoint counts off disk (`with_upscale_tracking`) for dual
  progress bars and resumability. Success/failure today is inferred by output-file
  existence (no exit-code inspection yet — an ongoing improvement, see the
  native-port work). An upscale job outlives the GUI that started it — `on_exit`
  doesn't cancel it, and a later vhs-gui process can find one still running via
  `PipelineJob::check_lock`/`attach_running` (reads `vhs_upscale.sh`'s own
  `WORK_DIR/upscale.pgid`, written by the script or the native worker) and reattach for progress display and Cancel, instead of
  racing a second Real-ESRGAN process against the same checkpoint segments.
  `check_lock` only trusts the pid if it is a process-group leader whose
  `/proc/<pid>/cmdline` names a `vhs_upscale*.sh` script or the `upscale-worker` subcommand (a SIGKILLed script leaves
  a stale pgid file whose pid can be reused); a failing lock is deleted. Reattach
  takes the segment length from the running job's `WORK_DIR/run_config.txt`
  (`SEG_SECONDS`), not the UI slider.
- **`src/panels/monitor.rs`** + **`src/capture.rs`** — capture state machine (`Idle →
  Monitoring → Releasing → Capturing`). mpv never opens `/dev/video`: `monitor_feed::MonitorFeed` is
  a small ffmpeg that reads the grabber and sends the preview mpv plays over UDP; Releasing waits
  until it has exited (mpv's `idle` is not a signal that the device is free). Closing mpv's own V4L2
  source crashed the GUI (libavdevice buffer-release callback after the device closed; see
  ROADMAP.md), which is why.
  `CaptureController` spawns ffmpeg directly (formerly `vhs_capture_ffmpeg.sh`) in its own
  process group, with stderr going straight to `logs/<prefix>_<ts>.ffmpeg.log` so a capture and
  its log survive the GUI dying. `CaptureSettings` holds the hardware/codec settings: compiled-in
  defaults equal to the shipped archival `capture.env`, with the active slot's
  `~/Videos/ffmpeg-current/capture.env` applied over them. `ffmpeg_args` is pure and compared with
  the real script by the ignored `matches_the_bash_script` test (read its safety notes before
  changing it: it once started a real capture). Capture length is enforced by a re-armable SIGINT
  timer (`arm_stop_timer`) that only signals the live process group (`signal_if_live`): the "Cap"
  field arms it at capture start and "Stop after"/Set re-arms it up or down at any time. ffmpeg
  itself runs with a fixed 6 h `-t` backstop (`FFMPEG_HARD_SAFETY_CAP`) because its own `-t` can't
  be changed once running; "Cap" is disabled while capturing. Progress is read from the last 8 KiB
  of the log, split on `\r` as well as `\n`. A capture outlives a GUI crash by design (own process
  group, 6 h backstop), so `find_orphans` scans `/proc` for capture ffmpegs this process did not
  start (program `ffmpeg`, `-f v4l2` input, `.mkv` output); the monitor panel shows a banner with
  Reattach (`CaptureController::adopt`, group leaders only) and Stop it, and replaces Monitor/Start
  Capture while one is running because it holds the grabber. `logs/capture.pgid` is written for
  manual stops (`kill -INT -$(cat ~/Videos/logs/capture.pgid)`).
- **`src/panels/upscale.rs`** — file library, per-`FileKind` action buttons
  (`launch_pipeline`/`launch_upscale`), upscale settings panel, before/after preview.
- **`src/library.rs`** — categorizes files into Viewer / EditMasterVD / EditMaster /
  Stabilized / Archival by directory + filename pattern (no probing).
- **`src/v4l2.rs`** — the native-Rust precedent to imitate for future ports: raw
  `libc::ioctl` (`VIDIOC_G_CTRL`/`VIDIOC_S_CTRL`), no subprocess, fd owned by a
  dedicated background thread reached via a bounded channel so the UI thread never
  blocks on a slow USB control transfer. This replaced an earlier `v4l2-ctl`
  subprocess-based implementation.
- **`src/config.rs`** — all hardcoded paths (data dirs, transitional script paths,
  `upscale_work_root()` on a separate scratch drive).
- **`src/settings.rs`** / **`src/persist.rs`** — upscale knobs (backend, model,
  scale, CRUSH/BRIGHTNESS presets) and TOML settings persistence. A model with a
  fixed native scale (e.g. `VHS-Sharpen-1x`) locks the internal scale, and
  `clamp_final_scale` keeps Final Scale something the scripts accept (<= and a
  divisor of the internal scale); both run at launch, not just in the panel.
- **`src/tools.rs`** + **`src/panels/tools.rs`** — the single resolver for external tools
  (`Tool`, `resolve`, `require`, `command`; order: `VHS_GUI_<TOOL>` override, bundled dir,
  `/usr/bin`, `~/bin`+`PATH`) and the background startup check behind the Tools window and the
  toolbar warning. Never hardcode `/usr/bin/<tool>` or call `Command::new("ffmpeg")`; use
  `tools::command(Tool::X)` or `tools::require(Tool::X)?` before spawning. `/usr/bin` precedes
  `PATH` on purpose: `/usr/local/bin/ffmpeg` is the DeckLink build and must not be picked up here.
- **`src/denoise.rs`** — native port of `vhs_denoise.sh` + `denoise.sh` (Denoise button): a three-step
  `SeqStep` sequence (extract WAV, `sox norm -2`, remux) built by the pure `steps`, sharing
  `extract_full_args`/`remux_args` with `audio_cleanup.rs`. Denoise+QTGMC is this job with QTGMC chained
  after it by `UpscalePanel::chain_after` (see `chained_next`). Its work files (and Audio Cleanup's) go in a hidden dir beside the output, not `/tmp`, which is
  RAM-backed (`audio_cleanup::unique_work_dir`); every sox step goes through `checked_sox_step` because
  SoX_ng exits 0 after `sox FAIL`. The ignored `matches_the_bash_script`
  test compares against the real script with stub tools; rerun it after changes.
- **`src/upscale/`** — the native upscale engine (Script/Native setting, default Script; native for all
  three variants). `plan.rs` is pure: segment math, output size, filter chains, the
  `run_config.txt` fingerprint (byte for byte as the scripts write it) and every command's arguments.
  `engine.rs` runs them (`Binaries` are injected so tests use stubs); a segment is encoded into
  `seg_part/` and renamed into `segments/` only after its frame count checks out. `worker.rs` is
  `vhs-gui upscale-worker JOB.toml`, decided in `main()` before any GUI setup: it runs detached in its
  own process group (`upscale::start_worker`, via `/proc/self/exe`), writes `upscale.pgid`, refuses a
  second worker on the same work dir, and treats SIGINT/SIGTERM as cancel (exit 130). Pause/resume are
  SIGSTOP/SIGCONT to the group. `testkit.rs` + `tests/support/stub.py` are stub ffmpeg/ffprobe/upscaler
  programs; the ignored golden test compares the engine with the real script, and
  `tests/upscale_equivalence.rs` (ignored, needs the GPU) compares real outputs. Read
  `docs/upscale-engine-design.md` before changing any of it.
- **`src/viewer_encode.rs`** — native port of `vhs_viewer_encode.sh` (Viewer Encode button). `plan` picks
  the SD/VHS or HD preset from the source height, `ffmpeg_args` builds the argument list as a pure
  function, `launch` returns `Started(job, output)` or `Exists(path)` (an existing viewer copy is never
  overwritten). The ignored `matches_the_bash_script` test runs the real script with stub ffmpeg/ffprobe
  and compares arguments; rerun it after any change here.
- **`src/monitor_feed.rs`** — the live monitor's source: `feed_args` (the capture's video input plus
  its UDP preview encode) and `MonitorFeed` (own process group; `PR_SET_PDEATHSIG` so the kernel
  terminates it if the GUI dies, unlike a capture; `stop` = SIGINT, `kill` = SIGKILL, reaped by
  `poll`). Ignored hardware tests check the stream and the monitor-capture-monitor handoff on the
  real grabber (GUI must be closed).
- **`src/vs_stack.rs`** — presence check (not import) for the VapourSynth plugins and the havsfunc
  Python modules the scripts need; feeds the Tools window and toolbar warning. Keep `NEEDS` in step
  with the `vhs_plugins.load(...)` calls in `assets/vs/*.vpy` and the modules `havsfunc` imports.
- **`tools/realesrgan-rocm/`** — source of the ROCm upscaler (`driver.py`, `requirements.txt`, the
  `realesrgan-rocm` shim). Installed by hand into `~/opt/realesrgan-rocm` and `~/bin` per
  `docs/INSTALL.md`; the app calls the installed shim, not these files.
- **`src/vs_assets.rs`** + **`assets/vs/`** — the VapourSynth scripts (`qtgmc`, `ivtc`, `ivtc_decombed`,
  `field_align`, `vdecimate` `.vpy` and the `vhs_plugins.py` plugin loader) embedded with
  `include_str!`. `materialize()` writes them to `$XDG_CACHE_HOME/vhs-gui/vs/<content-hash>/` and
  `vs_ops` runs vspipe from there with that dir first on `PYTHONPATH`. Plugins are looked up by file
  name (`VS_PLUGIN_DIRS` overrides), never by absolute path. `havsfunc` (Unlicense) and `vsutil` (MIT) stay
  external in `~/.local/share/vsrepo/py`; `vhs_plugins.prepare_havsfunc` makes havsfunc call znedi3 on R81 (see ROADMAP.md).
- **`src/mpv_view.rs`** — `MpvView` owns the leaked `&'static Mpv` and its
  `RenderContext`, GL thread only. Its `Drop` replaces the repaint callback with a
  zero-sized no-op before the context is freed: `libmpv2` frees the callback's
  closure *before* `mpv_render_context_free`, and mpv can still fire it from a
  decoder thread meanwhile (this segfaulted the app on exit). Keep the no-op
  zero-sized.

## Transitional bash dependency

`src/config.rs` still has `*_script()` accessors pointing at
`~/Videos/vhs-cli/vhs_*.sh` for operations not yet natively ported. Each is marked
`// TODO(native-port): remove once <op> is ported`. When porting an operation:
extend `PipelineJob` with new native-spawn constructors rather than a parallel
struct (the GUI already renders progress/pause/cancel off `Option<PipelineJob>` —
keep that surface stable), spawn the underlying tool(s) directly with
`.process_group(0)` semantics preserved, and remove the corresponding `*_script()`
accessor once its last caller is gone.

## Key Design Patterns

- **Time-based progress**, not frame-based (see `pipeline.rs`).
- **`.process_group(0)`** on every spawned job so signaling a job's process group
  never reaches the GUI process itself.
- **No PGID files for directly-held children.** `PipelineJob` and `CaptureController` signal
  via the live `Child`'s pid directly. A PGID file (a bash script's `trap ... EXIT`) is only
  *read* by the GUI, and only for a reattached upscale job (see `pipeline.rs` above), where the GUI
  holds no handle because the job outlived the process that started it. The native upscale worker
  writes its own `upscale.pgid` (it is the job, not the GUI) and `check_lock` accepts either kind.
- **`~/bin` is prepended to PATH** on every spawn so user-installed tools
  (`realesrgan-rocm`, etc.) resolve even from a desktop-launched session.
- **Work dirs are deleted only after output-file existence is confirmed** — the
  de-facto success check throughout, in the absence of exit-code inspection.
- **`on_exit` doesn't cancel the upscale job.** It used to, but that made the
  GUI's own stability a single point of failure for a multi-hour GPU job —
  `on_exit` fires on any event-loop shutdown, not just a deliberate quit, and
  can't tell the two apart. The job is process-group-isolated and
  checkpoint-resumable already, so leaving it running is strictly safer; Cancel
  and Stop-after-Segment in the UI remain the explicit way to stop one.

## Versioning

Tags: `v0.1.0` (single-view build), `v0.2.0` (dual-panel Monitor/Upscale rework),
`v0.3.3` (upscale jobs survive/reattach across GUI restarts), `v0.3.4` (native
Audio Cleanup for Edit Master/VD/Viewer -- hum notch, noisered, two-pass
loudnorm, and a "Find Quiet Spot" noise-sample-window scan; see
`start_native_sequence`/`SeqStep` in `pipeline.rs` for the underlying job shape),
`v0.3.5` (live-extendable capture Cap timer with a 6 h ffmpeg backstop, hardened
upscale reattach, Final Scale clamped to the model's internal scale, and a fix for
the segfault on exit from a freed mpv update callback).
System install target: `/usr/local/bin/vhs-gui`, launched via
`~/.local/share/applications/vhs-gui.desktop` (`install-desktop.sh` manages both).
Release process: `cargo build --release` → `sudo install` → bump `Cargo.toml` → tag.

`[profile.release]` sets `strip = true` — the installed production binary has no
symbols. A crash address from `dmesg`/`journalctl` won't resolve against it. To
debug a real crash, build a temporary symbol-retaining binary (unset `strip`, or
use the debug profile) rather than trying to resolve addresses against the stripped
release build.
