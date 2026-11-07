# Roadmap: a self-contained, portable vhs-gui

Goal: one executable that needs no vhs-cli checkout and no bash scripts, shipped as a portable
package. Work is done in phases, one branch and PR per phase. Tick a box in the same PR that
finishes it. Last updated 2026-11-07.

## What "self-contained" means here

- **In scope:** no `bash <script>` call sites; the VapourSynth scripts embedded in the binary;
  every external tool found through one resolver; a package that ships ffmpeg-independent pieces
  (binary, desktop entry, the plugin files that have no wheel) and sets up the rest.
- **Stays external, checked at startup:** VapourSynth itself, the Real-ESRGAN ROCm/torch venv and
  the ncnn binary, model weights, GPU drivers. Too large, GPU-specific or licence-entangled to
  bundle. A missing tool is reported, not guessed at.

## Branching and rules

- Long-lived branch `feat/self-contained` (cut from `main`); each phase is a short branch with a PR
  into it; one final PR takes it into `main` for a release. Merge `main` into it when `main` moves.
- Extend `PipelineJob` (`start_native_single`, `start_native_pipe`, `start_native_sequence`); no
  parallel job struct, no new `Command::new("bash")`. Keep `.process_group(0)`, time-based
  progress, and the codec policy (FFV1/PCM intermediates, H.264/AAC viewer copies).
- Golden test per port (real script with stub tools vs the Rust builder; a mutation must make it
  fail), plus a check on a short real file. `cargo fmt --check`, `cargo clippy --all-targets`,
  `cargo test`; README and CLAUDE.md updated in the same PR. vhs-cli stays the reference.
- Tests that run a real script must neutralize every file it sources and every hardware path (temp
  `HOME`, stub tools, hard time limit, own process group, assert the stub was called): the capture
  golden test once started a real capture.

## Done

- [x] **Phase 0** `src/tools.rs`: one resolver (`VHS_GUI_<TOOL>` override, bundled dir, `/usr/bin`,
      `~/bin`+`PATH`); Tools window and toolbar warning; every hardcoded path replaced.
- [x] **Phase 1** the five `.vpy` scripts and `vhs_plugins.py` embedded (`src/vs_assets.rs`, written to
      `$XDG_CACHE_HOME/vhs-gui/vs/<hash>/`); plugins found by file name. Tried on a real capture
      (2026-11-03, R55): QTGMC, IVTC, IVTC+Decomb, Field Align, Fix A/V Sync. Not tried in the GUI:
      VDecimate (needs an `EDIT_MASTER*.mkv`) and IVTC+Decomb's combed-frame branch on film.
- [x] **Phase 1b** `docs/INSTALL.md` (each step marked tested or not); ROCm upscaler under version
      control in `tools/realesrgan-rocm/`; Tools window checks the VapourSynth stack (`vs_stack.rs`).
- [x] **Phase 2** Viewer Encode (`viewer_encode.rs`): SD/VHS or HD preset from the source height, an
      existing copy is never overwritten. Golden test (12 source shapes) and a GUI run on a real
      Edit Master. Also fixed: `prepare_spawn` never created the log dir. Not ported on purpose: the
      script's `MODE`/`DEINTERLACE`/`V_*`/`A_*` overrides and its 2-pass ABR branch.
- [x] **Phase 3** Denoise (`denoise.rs`, three `SeqStep`s: extract WAV, `sox norm -2`, remux) and
      Denoise+QTGMC (QTGMC chained after a successful denoise, `chain_after`). Golden test; tried in
      the GUI (a cancelled denoise never starts QTGMC). The denoise bar pulses (no single progress
      stream). Not ported: the script's env overrides and idet auto-decision.
- [x] **Phase 4** capture (`capture.rs`): compiled-in defaults equal to the archival `capture.env`
      with `~/Videos/ffmpeg-current/capture.env` applied over them; ffmpeg spawned directly in its own
      group with the 6 h backstop and the re-armable Cap timer; progress from the last 8 KiB of the
      log. Orphaned captures (GUI crash left ffmpeg recording) are found via `/proc` with
      Reattach / Stop it. The GUI crash when closing the V4L2 monitor (libmpv/libavdevice
      use-after-free) is avoided by never letting mpv open `/dev/video`: `monitor_feed.rs` is a small
      ffmpeg sending a UDP preview. Tried by Ryan in the GUI ("tests look good"); the monitor picture
      is now a 480x360 preview with a little more delay (`preview_scale` in `CaptureSettings`).
- [x] **Phase 5a-5c** native upscale engine (`src/upscale/`, design in
      `docs/upscale-engine-design.md`): detached `vhs-gui upscale-worker` so a job outlives the GUI;
      segment loop with `seg_part/` staging, frame checks, one auto-repair pass, concat, audio mux,
      sync fix, byte-exact `run_config.txt`. All three variants wired (Engine setting: Script default /
      Native beta). Golden tests per variant against the real scripts, worker integration tests (real
      binary, SIGSTOP/CONT/INT/KILL), lock/reattach/stop-after-segment/stale-work-dir tests; a GPU
      test where script and worker gave bit-identical output on an 8 s clip (plain variant only; Ryan
      has not run Anime or B&W on real material). Dead `settings.denoise` removed; `DECOMB` (anime)
      not ported.
- [x] **Fix 2026-11-04** Denoise and Audio Cleanup kept multi-GB WAVs in `/tmp` (RAM-backed) and trusted
      `sox`, which exits 0 after `sox FAIL`: a 4 h 45 min capture exhausted `/tmp`, `ffmpeg` buffered the
      whole video (11 GB) and was OOM-killed. Work files now sit in a hidden dir beside the output and
      every sox step is checked (`checked_sox_step`). Verified on the same capture (Denoise+QTGMC ran).
- [x] **Phase 6a, VapourSynth R81 groundwork** (2026-11-07). `vhs_plugins.load` skips files whose
      namespace the core already provides and searches `vapoursynth.get_plugin_dir()`; `vs_stack`
      accepts R81's plugin names and finds the folder next to a pip `vspipe`; `prepare_havsfunc` makes
      upstream havsfunc call `core.znedi3` when only znedi3 exists. R81 vs R55 on an 8 s slice
      re-interlaced from an edit master: QTGMC Slower PSNR 49.4 dB avg (min 48.0), SSIM 0.996;
      IVTC+Decomb PSNR 47.7 avg (min 31.2, combed-frame repair differs with the newer mvtools);
      VDecimate, IVTC and Field Align frame-identical. Licences: `havsfunc` Unlicense, `vsutil` MIT;
      `mvsfunc`, `nnedi3_resample`, `adjust` have none and are not needed (imported only in functions
      the app never calls), so they left `vs_stack::NEEDS`.

## Open

- [ ] **5d** After a real tape-length native upscale matches the script: default to native; remove the
      three `upscale*_script()` accessors, `scripts_dir`, the "Scripts dir" field,
      `PipelineJob::start` and its bash call site, and the "Transitional bash dependency" section of
      CLAUDE.md. Needs Ryan to run one.
- [ ] Anime and B&W native upscale on real material; a worker-level stop-after-segment then resume run.
- [ ] GUI test of VDecimate on an `EDIT_MASTER*.mkv`.
- [ ] Verify the INSTALL.md steps nobody ran: ncnn zip layout, model archive layouts, rebuilding the
      ROCm venv in a scratch location, a fresh VapourSynth install on 26.04 (vsrepo cannot run there:
      it needs a Python that imports `vapoursynth`, and R55 is a 3.12 build).
- [ ] Monitor and capture cases not recorded as tried: timed stop that resumes the monitor, cancel.
- [ ] Release from `feat/self-contained` (the installed `/usr/local/bin/vhs-gui` is still v0.3.5 and
      lacks the long-capture fix; use the debug build for long captures until then).

## Phase 6: packaging (proposal, awaiting Ryan's decision)

Facts (2026-11-07): the binary links only `libmpv.so.2` directly. The savoury1 PPA ships only the
VapourSynth core (`vapoursynth`, `vapoursynth-python3`, `vspipe`, two libraries), no plugins, so on any
Ubuntu the plugins come from elsewhere. Ubuntu 26.04 has no VapourSynth in its archive. PyPI has
R81/R82 (`abi3`, Python 3.14 ok, LGPL) and Linux wheels for `vapoursynth-mvtools` 29 and
`vapoursynth-znedi3` 3.3 (GPL-2.0), `vapoursynth-vivtc` 2.0; ffms2 and fmtconv have no Linux wheel and
miscfilters is not on PyPI (licences: fmtconv WTFPL, miscfilters LGPL-2.1, both archived; ffms2 shows
no SPDX id upstream; `bestsource` is MIT and active). R82 is out; only R81 was tested. `cargo-deb` is
not installed; `dpkg-deb`, `fakeroot`, `lintian` are.

Recommended shape, replacing the earlier "two packages" idea:

- [ ] One `.deb` built in a clean 24.04 container (check it runs on 26.04 here; otherwise one per
      series): binary, desktop entry, icon, licences, and `ffms2`/`fmtconv`/`miscfilters` built from
      pinned source in the container. `Depends: libmpv2, ffmpeg, sox`.
- [ ] A user-started "Set up VapourSynth" step in the Tools window: a private venv under
      `~/.local/share/vhs-gui/` with R81 (or R82 after a test), the three API 4 plugin wheels,
      `havsfunc` and `vsutil`; first-run `vapoursynth config` (writes
      `~/.config/vapoursynth/vapoursynth.toml`, keyed by `libvsscript.so`, coexists with R55). This
      drops the PPA and the leftover-`libpython3.12` dependency for new installs; Ryan's R55 setup
      keeps working because the loader handles both.
- [ ] Check the exact "or later" wording of the GPL-2.0 wheels against vhs-gui's GPL-3.0 before
      shipping them in a package.
- [ ] First step if approved: build the three plugins from pinned source in a 24.04 container and
      prove they load under R81.
- [ ] Run QTGMC on a real raw capture under R81 (none is left: the archival was deleted after
      editing, so a new capture is needed).
- [ ] Optional later: AppImage (linuxdeploy + appimagetool) with `libmpv.so.2`, ffmpeg, ffprobe and
      sox bundled (GPL: ship a sources note), `scripts/package-appimage.sh`, attach to the release,
      test with `/usr/bin/ffmpeg` hidden; Wayland and GL under AppImage can need quirks.
