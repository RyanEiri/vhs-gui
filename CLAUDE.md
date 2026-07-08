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
  native-port work).
- **`src/panels/monitor.rs`** + **`src/capture.rs`** — capture state machine (`Idle →
  Monitoring → Releasing → Capturing`), V4L2 device handoff between mpv and ffmpeg,
  PGID-file-based signal coordination (capture's flow doesn't hold a direct child
  handle at signal time, unlike `PipelineJob`).
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
  scale, CRUSH/BRIGHTNESS presets) and TOML settings persistence.

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
- **No PGID files for directly-held children.** `PipelineJob` signals via the live
  `Child`'s pid directly; PGID files (`logs/*.pgid`, written by bash scripts'
  `trap ... EXIT`) are only needed where the GUI doesn't hold a direct handle at
  signal time (`capture.rs`'s flow).
- **`~/bin` is prepended to PATH** on every spawn so user-installed tools
  (`realesrgan-rocm`, etc.) resolve even from a desktop-launched session.
- **Work dirs are deleted only after output-file existence is confirmed** — the
  de-facto success check throughout, in the absence of exit-code inspection.

## Versioning

Tags: `v0.1.0` (single-view build), `v0.2.0` (dual-panel Monitor/Upscale rework).
System install target: `/usr/local/bin/vhs-gui`, launched via
`~/.local/share/applications/vhs-gui.desktop` (`install-desktop.sh` manages both).
Release process: `cargo build --release` → `sudo install` → bump `Cargo.toml` → tag.
