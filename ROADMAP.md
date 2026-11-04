# Roadmap: a self-contained, portable vhs-gui

Goal: one executable that needs no vhs-cli checkout and no bash scripts, shipped as a portable
package. Work is done in phases, one branch and PR per phase, whenever time allows. Tick a box in
the same PR that finishes it.

## What "self-contained" means here

- **In scope:** no `bash <script>` call sites; the VapourSynth scripts embedded in the binary;
  every external tool found through one resolver; a portable package that bundles ffmpeg, ffprobe,
  sox and libmpv.
- **Stays external, checked at startup:** VapourSynth (`vspipe`, plugins, Python 3.12 runtime), the
  Real-ESRGAN ROCm/torch venv and the ncnn binary, model weights, GPU drivers. They are too large,
  GPU-specific or license-entangled to bundle. A missing tool is reported, not guessed at.

## Where things stand

`src/config.rs` marks every remaining script with `TODO(native-port)`:
capture (`vhs_capture_ffmpeg.sh` + `capture.env`), Denoise (`vhs_denoise.sh`, `denoise.sh`),
Denoise+QTGMC (`vhs_process.sh`), Viewer Encode (`vhs_viewer_encode.sh`), and three upscale variants
(`vhs_upscale.sh`, `_anime`, `_bw`). Five `.vpy` files and `havsfunc.py` are read from the vhs-cli
checkout. Tool paths are hardcoded (`/usr/bin/ffmpeg`, `/usr/bin/ffprobe`, `/usr/bin/sox`,
`~/bin/realesrgan-rocm`, `~/.local/share/vsrepo/py`).

## Branching

All work for this project lands on the long-lived branch `feat/self-contained`, cut from `main`.
Each phase is a short branch named `feat/self-contained-<phase>` (for example
`feat/self-contained-tools`) with a PR into `feat/self-contained`. When the branch is ready for a
release, one PR takes it into `main`. Merge `main` into it when `main` moves (bug fixes, releases).

## Rules for every phase

- Extend `PipelineJob` (`start_native_single`, `start_native_pipe`, `start_native_sequence`); no
  parallel job struct, no new `Command::new("bash")`.
- Keep `.process_group(0)`, time-based progress, and the codec policy (FFV1/PCM intermediates,
  H.264/AAC viewer copies).
- Golden test per port: the ffmpeg argv built in Rust matches what the script builds; check output
  streams with `ffprobe` on a short real file.
- `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`; update README and CLAUDE.md in
  the same PR. vhs-cli stays the reference implementation and is not changed.

## Phase 0: roadmap and groundwork

- [x] `ROADMAP.md`, linked from the README.
- [x] `src/tools.rs`: one resolver for ffmpeg, ffprobe, sox, vspipe, realesrgan-rocm,
      realesrgan-ncnn-vulkan. Order: `VHS_GUI_<TOOL>` override, bundled dir (`$APPDIR/usr/bin`,
      `<exe>/../lib/vhs-gui/bin`), `/usr/bin`, `~/bin` + `PATH`. Every hardcoded path in
      `fix_sync.rs`, `audio_cleanup.rs`, `vs_ops.rs` and `pipeline.rs` now goes through it.
      Unit tests cover the order. The override is an environment variable; a GUI settings
      field for it is not built.
- [x] Startup dependency check: a background check feeds a Tools window (rail button) with
      found/missing, path and version per tool, and a toolbar warning when something needed is
      missing. `ffmpeg`, `ffprobe`, `sox` and `vspipe` fail with an actionable message before a
      job spawns. The upscale scripts still find `realesrgan-*` on their own; a preflight for
      them comes with Phase 5.
- [x] Fix the clippy `chunks_exact` warning at `src/main.rs:29`.

## Phase 1: embed the VapourSynth scripts

- [x] The five `.vpy` scripts and a small `vhs_plugins.py` loader live in `assets/vs/` and are
      `include_str!`-ed (`src/vs_assets.rs`). At job start they are written to
      `$XDG_CACHE_HOME/vhs-gui/vs/<content-hash>/` (rewritten only if missing or different) and that
      directory goes first on `PYTHONPATH`.
- [x] Plugins are found by file name in `~/.local/share/vsrepo/plugins` and the system VapourSynth
      dirs (`VS_PLUGIN_DIRS` overrides), replacing the hardcoded `/home/ryan/...` paths. This
      departs from the first plan, which kept absolute paths and would not have run on any other account.
- [x] `Config::tools_dir` removed; QTGMC, IVTC, VDecimate and Field Align no longer read the vhs-cli
      checkout. Frame hashes of the embedded scripts match vhs-cli's for QTGMC, IVTC, Field Align
      and VDecimate on a test clip.
- [x] **Tested on a real capture through the GUI (2026-11-03, debug build, R55).** QTGMC, IVTC,
      IVTC+Decomb, Field Align and Fix A/V Sync all ran to completion on a short denoised capture and
      the results looked right; the embedded scripts were the ones executed (`vspipe` pointed at
      `~/.cache/vhs-gui/vs/<hash>/`). IVTC+Decomb no longer hangs. Not tested: VDecimate (needs an
      `EDIT_MASTER*.mkv`), and IVTC+Decomb's combed-frame branch on telecined film.
- [ ] **Decision: embed the havsfunc stack?** Not done. The copy in vhs-cli is not the one that
      runs (it calls `core.znedi3` and has a syntax error); the working one is the vsrepo copy in
      `~/.local/share/vsrepo/py`. Upstream licences: `vsutil` MIT, `havsfunc` Unlicense,
      `mvsfunc`, `nnedi3_resample` and `adjust` no licence detected. Embed only after the authors
      confirm a licence; otherwise keep it external (the plugin binaries stay external either way).

## Phase 1b: document and check the external tools

- [x] `docs/INSTALL.md`, linked from the README: install steps for ffmpeg, sox, libmpv, VapourSynth
      (`ppa:savoury1/vapoursynth`, and `apt-mark manual` for the auto-installed `libpython3.12`
      packages that `apt autoremove` would remove), the vsrepo plugins and modules,
      `realesrgan-ncnn-vulkan`, and the model archives. Each step is marked tested or not.
- [x] The ROCm upscaler is under version control in `tools/realesrgan-rocm/` (`driver.py` and the
      shim copied byte for byte from the working install; `requirements.txt` updated to the
      `rocm7.1` index and the installed versions), with venv, PyTorch wheel, group and
      `HSA_OVERRIDE_GFX_VERSION` steps. The venv was not rebuilt from scratch to prove the steps.
- [x] Tools window: a VapourSynth section (`src/vs_stack.rs`) checks that the six plugins, the nnedi3
      weights file and the five Python modules exist, and says which operations need each.
- [ ] **Fresh VapourSynth install on Ubuntu 26.04 has no tested path.** vsrepo needs a Python that
      imports `vapoursynth`; R55 is a Python 3.12 build and 26.04 has no 3.12 interpreter, so
      `vsrepo.py` stops with "Could not detect vapoursynth". Also, vsrepo's current `havsfunc` lists
      `znedi3` where the installed copy uses `nnedi3`, so a fresh install may fetch a `havsfunc` the
      scripts do not match. Settle together with the R55 to R81 migration (memory note
      `vapoursynth-r55-python312`), then rewrite section 2b of INSTALL.md from a clean run.
- [ ] Verify the ncnn zip layout, the model archive layouts, and rebuild the ROCm venv in a scratch
      location to confirm the INSTALL.md commands as written.

## Phase 2: Viewer Encode (smallest script)

- [x] `vhs_viewer_encode.sh` ported to `src/viewer_encode.rs`: one native ffmpeg job. Height <= 576
      gets the SD/VHS preset (x264 CRF 18, fast, main@4.0, 640x480 Lanczos, 29.97); taller gets HD
      (CRF 20, medium, high@4.1, no scale, CFR from the probe). AAC-LC 320k stereo 48 kHz. An existing
      viewer copy is left alone, as before. The argument builder is a pure function.
- [x] Golden test: with stub `ffprobe`/`ffmpeg` the real script and the Rust builder produce identical
      ffmpeg arguments for 12 source shapes (`cargo test -- --ignored`, needs the vhs-cli checkout).
      An end-to-end smoke test runs real ffmpeg on a generated clip and checks the codecs.
- [x] `viewer_encode_script()` removed. Not ported, deliberately: the script's `MODE`, `DEINTERLACE`,
      `V_*` and `A_*` environment overrides and its 2-pass ABR branch (the GUI never set them; ABR was
      unreachable from the GUI).
- [x] Found on the way and fixed separately: `prepare_spawn` never created the log directory, so every
      job failed with a bare "No such file or directory" on a machine without `~/Videos/logs`.
- [x] Tried the Viewer Encode button in the GUI on a real Edit Master file (2026-11-03, debug build):
      SD preset as designed (H.264 Main@4.0, 640x480, yuv420p, 29.97 fps; AAC-LC 48 kHz stereo; same
      duration as the source), plays fine, and a second click left the existing copy alone. Not
      tried: the HD preset on a real source (covered by the argument-matching test only).

## Phase 3: Denoise and Denoise+QTGMC

- [x] `vhs_denoise.sh` + `denoise.sh` ported to `src/denoise.rs` as a three-step `SeqStep` sequence:
      extract audio to WAV, `sox norm -2`, mux with the video copied bit-exact and PCM audio. It runs
      the script's defaults, which means `NOISERED_ENABLE=0`: the shipped "denoise" is a 20 Hz
      high-pass, a timestamp rebase and 48 kHz stereo, plus the normalize (Audio Cleanup is the
      noise-reduction tool). The extract and mux commands are shared with `audio_cleanup.rs`
      (`extract_full_args`, `remux_args`). Output `captures/stabilized/<stem>_<HH-MM-SS>_STABLE.mkv`.
- [x] Denoise+QTGMC is a Denoise job with the native QTGMC job chained after it: when the denoise
      succeeds the panel starts QTGMC on its output (`chain_after`, `chained_next`), and a failed or
      cancelled denoise never does. This gives each stage its own progress bar and reuses the tested
      `vs_ops` pipe. An input already named `*_STABLE.mkv` goes straight to QTGMC, as the script did.
- [x] Golden test: with stub `ffmpeg`/`sox`, the real `vhs_denoise.sh` and the Rust steps make the same
      calls. One deliberate difference, stated in the test: the script copies `full.wav` to
      `clean.wav` before normalizing (noise reduction off); we normalize `full.wav` directly and skip
      that copy (about 1.3 GB for a 2 h tape). End-to-end test on a generated clip: video kept, 48 kHz
      stereo PCM, peak at -2 dBFS, scratch directory removed.
- [x] `denoise_script()`, `process_script()` and the now-unused `launch_pipeline` removed. Not ported:
      the `DENOISE_PRESET`/`NOISE_*`/`NR_AMOUNT`/`NORM_DB`/`FFMPEG_THREADS`/`REBASING_MODE`,
      `FORCE_*`, `REDO_*`, `SKIP_QTGMC`, `QTGMC_FRAMES` and `VPY` overrides, the idet auto-decision
      (the script's `FORCE_QTGMC` defaulted to 1, so it never ran), and the dormant `noisered` branch.
- [x] Tried in the GUI on a real archival capture (2026-11-03, debug build): Denoise ran its three
      stages, Denoise+QTGMC started QTGMC by itself after the denoise, and cancelling a
      Denoise+QTGMC mid-denoise did not start QTGMC (log stops at the first stage, no QTGMC log
      follows). Known differences: the Denoise bar pulses (a sequence has no single progress
      stream), and the QTGMC stage uses `vs_ops`' ffmpeg arguments (`-shortest`, explicit y4m
      input) rather than the script's.

## Phase 4: capture

- [x] `CaptureSettings` in `src/capture.rs`: the shipped archival `capture.env` compiled in as
      defaults (a test checks they match the file exactly), with `~/Videos/ffmpeg-current/capture.env`
      applied over them when present, so slot switching with `vhs_mode.sh` keeps working and a fresh
      install needs no file. The old `VHS_FFMPEG_BIN`/`VHS_FFPROBE_BIN` keys are ignored (the tool
      resolver replaced them).
- [x] ffmpeg is spawned directly and the `Child` held: own process group, stderr straight into
      `logs/<prefix>_<timestamp>.ffmpeg.log` (as `tee` did), 6 h `-t` backstop and the re-armable
      Cap timer kept. The pgid file is gone; the timer signals only the live process group
      (`signal_if_live`, unit tested). Output names are the script's `<prefix>_<timestamp>.mkv`.
- [x] Progress now reads the last 8 KiB of the log and splits on `\r` (ffmpeg's separator) as well as
      `\n`; the old code read the whole log on every poll and split on `\n` only.
- [x] Golden test: the real `vhs_capture_ffmpeg.sh` run with stub ffmpeg and a temp `HOME` against
      both shipped slots (archival FFV1, viewer libx264) produces identical ffmpeg arguments. The
      controller is tested against a stub ffmpeg that handles SIGINT (start, progress, stop, timer
      fires, cancelled and re-armed timers, natural exit); those tests were flaky (about 1 run in 4)
      from a "Text file busy" race between parallel tests and now retry that one error.
- [x] `capture_script()` and `capture_pgid_file()` removed. Not ported: the script's closing
      `ffprobe` listing and its "is ffmpeg still running" cleanup (the child is held directly).
- [x] Orphaned captures (found 2026-11-03: the GUI crashed at capture start twice and each time the
      detached capture ffmpeg kept recording, holding the grabber so the monitor would not start).
      `find_orphans` scans `/proc`; the monitor panel shows "A capture is already running" with
      Reattach and Stop it; `logs/capture.pgid` is written again so a capture can be stopped by hand.
      Tested with a real stand-in process (found, adopted, stopped).
- [x] **GUI crash when the live V4L2 monitor is closed: fixed by not letting mpv open the device
      (2026-11-03).** Original finding (pre-existing, not from this port):
      Core dumps: the installed v0.3.5 on 10-30 and 11-02, and the debug build on 11-03 (twice, at
      capture start). Same stack every time on mpv's demuxer thread: `av_packet_unref` ->
      `av_buffer_unref` -> a libavdevice free callback -> jump to a bad address; no vhs-gui frames.
      A scratch harness (libmpv2 with the app's V4L2 options, no-op video output, 15 runs per variant)
      shows libavdevice's buffer-release callback running against the closed device on every stop
      (`ioctl(VIDIOC_QBUF)` errors, 2 per run, 90 of 90 runs) but not crashing without the GUI's
      memory activity, and that mpv's `idle` is true immediately after `stop` while the grabber is
      still held (ffmpeg started right after idle got "Device or resource busy" 15 of 15 times; 1 s
      later 0 of 15). So a delay before starting ffmpeg does not remove the bug.
      Fix: `src/monitor_feed.rs`. The monitor is a small ffmpeg process sending the capture's UDP
      preview; mpv only plays UDP, so no V4L2 packet ever reaches it. Start Capture stops the feed
      and waits until it has exited before starting the capture; a timed stop resumes the monitor
      only after the capture has exited; the feed dies with the GUI (`PR_SET_PDEATHSIG`). Tests
      with the real grabber, GUI closed: the feed streams valid MPEG-TS to the preview port and
      releases the device, and monitor, then a 3 s capture, then monitor again works. The mpv and
      egui side (picture quality, latency, the buttons) is verified in the GUI by hand.
- [x] Monitor and capture tried in the GUI by Ryan (2026-11-03, debug build of 15:01, after the monitor
      fix): reported working ("tests look good"). Which cases were run is not recorded here; the
      cases to cover are Monitor then Start Capture then Stop, a timed stop that resumes the monitor,
      and a cancel. The monitor picture is now the scaled 480x360 preview with a little more delay
      than mpv's direct view, which could be revisited (`preview_scale` in `CaptureSettings`).

## Phase 5: upscale (largest, riskiest)

Design and the decisions it needs from Ryan: [`docs/upscale-engine-design.md`](docs/upscale-engine-design.md).
In short: a detached `vhs-gui upscale-worker` process (so a job still outlives the GUI), a faithful
port of the three scripts (they differ only in model, pre-filter, drift tolerance and a GUI-unreachable
decomb option) with the bash scripts kept selectable until a real upscale matches.

- [x] **5a** (built and tested 2026-11-03; see section 6 of the design doc) `src/upscale/` engine core and worker for the standard variant: segment loop, frame and
      segment verification, validation with one auto-repair pass, concat, audio mux, sync fix,
      `run_config.txt` byte for byte, `upscale.pgid` written by the worker, `.part` rename so a killed
      encode cannot poison a resume. Settings toggle "Upscale engine" (default: script). Golden tests
      against a patched copy of the real script with stub tools; a GPU equivalence test.
- [x] **5b** anime and B&W variants; all six upscale buttons wired (golden tests pass; GPU clip check for these two still to do).
- [ ] **5c** `check_lock`/`is_upscale_group_leader` accept the worker as well as `vhs_upscale*.sh` for one
      release; stale work-dir handling; resume, cancel, pause, stop-after-segment and reattach tests.
      Update the CLAUDE.md rule that Rust only reads PGID files.
- [ ] **5d** After a real upscale matches: default to the native engine; remove the three
      `upscale*_script()` accessors, `scripts_dir`, the "Scripts dir" field, `PipelineJob::start` and its
      bash call site, and the "Transitional bash dependency" section of CLAUDE.md.
- [x] Dead setting removed: `settings.denoise` and its "Denoise" checkbox in the upscale settings (it
      exported `DENOISE=1`, which no script and not the ROCm driver reads; correction: the checkbox did
      exist in the UI, an earlier note here said it did not). `DECOMB` (anime) is unreachable from the
      GUI and is not ported; it can return as an option later.

## Phase 6a: two packages, Ubuntu 24.04 (R55) and 26.04 (R81)

Idea: one source tree, two `.deb`s built per release in a clean container. The Rust binary is the
same; the packages differ in how VapourSynth is supplied. Spike results (2026-11-02, scratch venv,
nothing on the system changed):

- **24.04, R55, Python 3.12:** works today. `Depends:` on the `savoury1/vapoursynth` PPA packages (a
  `.deb` cannot add a PPA, so the docs must say so).
- **26.04, R81, Python 3.14:** Ubuntu 26.04's archive has no VapourSynth. R81 is a PyPI `abi3` wheel
  (6.4 MB, ships `vspipe` and the libraries, LGPL) that runs under Python 3.14 and could be bundled.
  R81 dropped API 3 plugins, so the vsrepo `libmvtools.so` and `libnnedi3.so` do not load. PyPI has
  prebuilt Linux wheels for the API 4 replacements: `vapoursynth-mvtools` 29 and
  `vapoursynth-znedi3` 3.3 (both GPL-2.0), and `vapoursynth-vivtc` 2.0. ffms2 and fmtconv have no
  Linux wheel on PyPI, and miscfilters is not on PyPI; the existing vsrepo builds of those three
  load under R81.
- **Measured:** QTGMC `Slower` on 8 s of real interlaced footage, R81 vs R55: PSNR 52.4 dB average
  (worst 48.9), SSIM 0.998, same speed (about 110 fps). Needed an installed `havsfunc` patched from
  `core.nnedi3.nnedi3` to `core.znedi3.nnedi3` (3 lines) and no explicit loads for the two plugins
  (R81 autoloads its own plugin folder). VDecimate, IVTC and Field Align were frame-identical to R55.

- [ ] `vhs_plugins.py` (and `vs_stack.rs`) also look in R81's plugin folder
      (`vapoursynth.get_plugin_dir()`); no behaviour change on R55.
- [ ] A `znedi3` variant of the QTGMC and IVTC+Decomb scripts for R81, chosen at runtime by the
      detected VapourSynth version.
- [ ] First-run `vapoursynth config` for a bundled R81 (writes `~/.config/vapoursynth/vapoursynth.toml`,
      keyed by `libvsscript.so` path, so it coexists with R55); a package install script cannot do it.
- [ ] Decide the `havsfunc` source for R81 (patched copy vs upstream 34, which needs `vsjetpack` and
      is untested) and confirm licences for `mvsfunc`, `nnedi3_resample` and `adjust`.
- [ ] Run QTGMC on a real VHS capture under R81 (the spike used a downscaled, interlaced slice of a
      DVR recording because the captures folder was empty).
- [ ] `cargo-deb` metadata and a clean-container build for each release; one `.deb` per series.

## Phase 6: portable package

- [ ] AppImage (linuxdeploy + appimagetool): binary, `libmpv.so.2` and its libraries, ffmpeg,
      ffprobe and sox with their libraries, icon and `.desktop`. `tools.rs` prefers the bundled dir.
- [ ] `scripts/package-appimage.sh` (reproducible), README install section, licenses file (the
      ffmpeg and sox builds are GPL: ship a sources note).
- [ ] Attach the AppImage to the GitHub release from the next tag.
- [ ] Test on the real desktop with `/usr/bin/ffmpeg` hidden (clean container or stripped `PATH`);
      Wayland and GL under AppImage can need quirks.
- [ ] Optional: a `.deb` through `cargo-deb` with `Depends:` instead of bundling.
