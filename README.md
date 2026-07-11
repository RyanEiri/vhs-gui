# vhs-gui — VHS Capture & Playback GUI

```
        █                                    ▀
 ▄   ▄  █ ▄▄    ▄▄▄           ▄▄▄▄  ▄   ▄  ▄▄▄
 ▀▄ ▄▀  █▀  █  █   ▀         █▀ ▀█  █   █    █
  █▄█   █   █   ▀▀▀▄   ▀▀▀   █   █  █   █    █
   █    █   █  ▀▄▄▄▀         ▀█▄▀█  ▀▄▄▀█  ▄▄█▄▄
                              ▄  █
                               ▀▀
```

[![License: GPLv3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
![Language: Rust](https://img.shields.io/badge/language-Rust-DEA584.svg)
![Platform: Linux](https://img.shields.io/badge/platform-Linux-lightgrey.svg)
[![Release](https://img.shields.io/github/v/release/RyanEiri/vhs-gui)](https://github.com/RyanEiri/vhs-gui/releases)

A native Rust desktop application that wraps VHS digitization — capture,
processing, playback, and AI upscaling — in a single window. It's the primary
day-to-day interface for tape digitization.

**This is a self-contained Rust application.** It does not call or depend on any
external pipeline scripts. Operations it performs (deinterlace, upscale, denoise,
etc.) are implemented natively in Rust, spawning only the underlying media tools
directly (ffmpeg, VapourSynth's `vspipe`, Real-ESRGAN) — never a wrapper script.
This is a deliberate, ongoing migration (see "Native-Rust port status" below); some
operations still shell out to scripts from the sibling
[vhs-cli](https://github.com/RyanEiri/vhs-cli) repo as a transitional measure while
that migration is in progress, tracked explicitly in `src/config.rs`.

There's also a bash/CLI version of this same pipeline,
[vhs-cli](https://github.com/RyanEiri/vhs-cli) — useful for terminal/scripted
workflows. The two are independent sibling projects; this repo has no dependency on
that one beyond the transitional script paths noted above.

**Expected local layout:** this repo lives at `~/Videos/vhs-gui/`, a sibling of the
data directories (`~/Videos/captures/`, `~/Videos/logs/`, etc.) which are not part of
this repo — they're local working directories referenced by absolute path from
`src/config.rs`.

## Hardware

Built and run on a single Linux workstation:

- **CPU:** AMD Ryzen 9 5900X (12-core).
- **GPU:** AMD Radeon RX 7800 XT (RDNA 3, gfx1101) — drives Real-ESRGAN upscale
  jobs via either backend (ROCm/PyTorch or Vulkan/ncnn; see the Upscale Settings
  panel). ROCm is required for the community VHS-specific models — the Vulkan
  binary segfaults on any model name outside its hardcoded family list.
- **VHS capture device (the flakiest link in the pipeline):** MacroSilicon
  MS210x USB video grabber — a low-cost USB2.0 analog capture dongle
  ("EasierCAP"-type). Opened via V4L2 at a stable `/dev/v4l/by-id/...` path
  (`src/config.rs`'s `v4l2_device` default) so it survives USB port changes;
  override it for different capture hardware. A fair amount of this app's
  capture-side code exists specifically to work around this device's quirks:
  - **Exclusive access.** Only one process can hold its V4L2 fd at a time, so
    handing off from the live Monitor preview (mpv) to a capture (ffmpeg)
    can't just start the second process — mpv's hold has to actually clear
    first. That's the `Releasing` capture state (1s timeout,
    `src/panels/monitor.rs`) between Monitoring and Capturing.
  - **Spurious preview drops.** The live-preview stream sometimes goes idle
    on its own mid-capture even though ffmpeg is still recording correctly
    in the background. Rather than surface that as an error, an "insurance
    reopen" (throttled to once per 2s, `src/panels/monitor.rs`)
    automatically reconnects mpv to the preview stream.
  - **Slow USB control transfers.** Setting brightness/contrast/hue/etc. via
    `VIDIOC_S_CTRL` can block for multiple seconds on this device. `src/
    v4l2.rs` issues every control write from a dedicated background thread
    over a bounded channel specifically so a slow ioctl never freezes the
    UI thread — see the comment above the thread spawn there.
  - Video and audio are two independent USB interfaces on the same dongle,
    not a hardware-synced A/V pair — they drift over a long capture. Native
    "Fix A/V Sync" corrects this after the fact.
- **Not used by this app, but worth noting:** the same machine also has a
  Blackmagic Design Intensity Pro PCIe capture card, used only by vhs-cli's
  separate OBS `game` env slot for console capture (unrelated to VHS
  digitization — this app never touches it). The cheap USB dongle above is
  the one that actually works; the "proper" dedicated capture card
  currently doesn't. Failure details and log evidence are in the
  [vhs-cli README](https://github.com/RyanEiri/vhs-cli#hardware), since
  that's the repo that owns the OBS config and logs it's diagnosed from.
- **Upscale scratch storage:** a secondary drive mounted at
  `/media/ryan/Patriot/Videos/vhs_upscale_work/` — segment checkpoints for
  chunked/resumable upscale jobs live there by default.

## Build & Launch

```bash
cd vhs-gui
cargo build            # debug
cargo build --release  # optimized, for system install

DISPLAY=:0 ./target/debug/vhs-gui           # run debug build
DISPLAY=:0 ./target/release/vhs-gui         # run release build
```

### System-wide install

```bash
cargo build --release
sudo install -m 0755 target/release/vhs-gui /usr/local/bin/vhs-gui
./install-desktop.sh   # installs the .desktop entry + icon, points Exec at /usr/local/bin/vhs-gui
```

## Stack

- `eframe`/`egui` 0.34 (glow/OpenGL backend, Wayland via winit)
- `libmpv2` 6.0.0 — render API, off-screen FBO, blitted via an egui paint callback
- `nix` — SIGINT/SIGSTOP/SIGCONT to process groups (pause/resume/cancel jobs)
- `image` (JPEG only) — decodes upscale preview frames
- `trash` — FreeDesktop.org trash (recoverable deletes)
- `png` — decodes the embedded window icon
- `serde` + `toml` — settings persistence (`~/.config/vhs-gui/config.toml`)

## What It Does

- **Monitor** — opens the V4L2 capture device directly in mpv for a live signal
  check before recording. The device is released cleanly once capture starts.
- **Start/Stop Capture** — spawns the capture process; the archival file opens in
  the embedded player as soon as it appears on disk, with a rolling near-live
  preview window. Ctrl+C/SIGINT during capture is a normal stop, not a failure.
- **Library panel** — five sections in display order: Viewer → Edit Master (VD) →
  Edit Master → Stabilized → Archival. Click any entry to open it in the player.
- **Pipeline actions** — per-section buttons launch Denoise, QTGMC, IVTC, VDecimate,
  Viewer Encode, and all upscale variants as background jobs. Only one job runs at a
  time; buttons are disabled while busy.
- **Upscale jobs** — dual progress bars (total segments / completed; upscaled frames
  / extracted frames for the active segment). Pause (SIGSTOP), Resume (SIGCONT),
  Stop after Segment (clean SIGINT at the next segment boundary), Cancel (immediate
  SIGINT). A side-by-side "Original vs Upscaled" preview updates every 4 seconds
  while a job runs.
- **Rename** — Viewer files get a title-suggestion field pre-filled from the
  filename (strips pipeline suffixes/prefixes, title-cases with acronym
  preservation, formats with an em dash).
- **Delete** — moves files to the FreeDesktop.org trash, recoverable from any file
  manager.

**Capture state machine:** `Idle → Monitoring → Releasing (1s timeout) → Capturing`.
The V4L2 device is exclusive to ffmpeg during capture; mpv returns to idle during
Releasing then switches to the growing archival file once capture begins.

## Native-Rust port status

`vhs-gui` is migrating away from shelling out to bash pipeline scripts, phase by
phase, toward being fully self-contained. Each phase replaces a handful of
bash-backed buttons with native Rust that spawns the underlying tools (ffmpeg,
`vspipe`, Real-ESRGAN) directly — never a wrapper script. `src/config.rs` marks each
remaining bash-backed operation with a `// TODO(native-port)` comment so the
remaining surface area is always visible in the source.

## System dependency notes

- Requires the `libmpv2` runtime.
- System `ffmpeg` at `/usr/bin/ffmpeg`.
- `realesrgan-rocm` shim at `~/bin/realesrgan-rocm` for the ROCm upscale backend.
- VapourSynth (`vspipe`) + `PYTHONPATH` pointing at `~/.local/share/vsrepo/py` for
  the deinterlace/telecine operations.
