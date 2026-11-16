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
      sync fix, byte-exact `run_config.txt`. All three variants wired (Engine setting, Script default /
      Native beta; the setting was removed in 5d). Golden tests per variant against the real scripts, worker integration tests (real
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
- [x] **Phase 5d, native upscale only** (2026-11-14). Real tape-length runs of the native engine: 20, 25
      and 30 min edit masters. Resume of a script-started work dir (the fingerprint check refused a
      changed brightness, as designed), pause/resume, Stop after Segment (clean boundary) and Cancel
      mid-segment (nothing partial left, the segment redone) all worked. The same test showed the
      script leaving a partial segment as a checkpoint after a mid-encode stop, which the native
      engine does not. Removed: the Script/Native setting, the three `upscale*_script()` accessors,
      `Config::scripts_dir` and its Working Directories field, `PipelineJob::start` and its bash call
      site, `UpscaleSettings::to_launch`, and the "Transitional bash dependency" section of CLAUDE.md.
      Old settings files with `engine` or `paths.scripts_dir` still load. The ignored golden tests
      keep using a sibling vhs-cli checkout as the reference (`config::vhs_cli_dir`); `check_lock`
      still recognises a `vhs_upscale*.sh` started from a terminal.
- [x] **Letterbox detection and crop** (2026-11-14, PR #22). `src/crop.rs`: Detect Letterbox under Edit
      Master rows, vertical crop only, applied to the native upscale and to Viewer Encode. Tested by Ryan
      on a letterboxed and a full-frame file: native upscale 1280x552, Viewer Encode 640x276. Later:
      pillarbox, a crop that changes inside one file, entering a crop by hand, remembering a detection
      across restarts.

## Open

- [ ] Anime and B&W native upscale on real material.
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

- [x] **6b, the `.deb`** (2026-11-15). `scripts/package-deb.sh` builds `dist/vhs-gui_<ver>-1_amd64.deb`
      for the release it runs on (26.04 here; the plugins link the distro's FFmpeg, so one package
      per release): binary, desktop entry, icon, copyright and the three plugin licences, and
      `ffms2`/`fmtconv`/`miscfilters` in `/usr/lib/vhs-gui/vs-plugins`, which the embedded scripts and
      `vs_stack` search after the user's own plugin folder. `Depends` come from `dpkg-shlibdeps` plus
      `ffmpeg, sox`. `lintian` shows two warnings (no man page, changelog wording). Unpacked and
      checked: layout, library resolution, plugins loading under R81 through `vhs_plugins.load`.
      Not done: installing it with `apt` (Ryan), a 24.04 build. ffms2's licence is confirmed: MIT
      source, with binaries that follow how FFmpeg was built.
- [x] **6c, "Set up VapourSynth"** (2026-11-15). A Tools window button builds a private venv in
      `~/.local/share/vhs-gui/vapoursynth` (`src/vs_setup.rs`): VapourSynth R81, `mvtools`, `znedi3`,
      `vivtc` and `vsutil` as wheels pinned by version and sha256 (`assets/vs/requirements.txt`,
      regenerated by `scripts/pin-vs-requirements.py`), then `havsfunc.py` downloaded from upstream at
      commit `40009447` and refused unless its sha256 matches, `vapoursynth config`, and an import
      check. `tools::resolve` finds its `vspipe` ahead of `/usr/bin`; Ryan's R55 keeps working. PyPI's
      havsfunc cannot be used: 33 needs the old `focus2` plugin, 34 needs `vsjetpack`. Upstream's last
      single-file havsfunc also builds `core.eedi3m` on every QTGMC call, so `vhs_plugins` answers a
      missing `eedi3m` with a stand-in that only fails if an EEDI3 mode is actually used. Verified in a
      26.04 container: the same steps, then QTGMC on a short interlaced clip (45 frames). Needs
      `python3-venv` and `libpython3.14`. Not done: pressing the button in the GUI here.
- [x] **Licences checked** (2026-11-15). `mvtools` and `znedi3` GPL-2.0, `vivtc` MIT (not GPL, as noted
      before), VapourSynth LGPL-2.1+, `vsutil` MIT, havsfunc Unlicense, ffms2 MIT source. Nothing is
      shipped or linked: the setup downloads them on request into a separate environment and runs
      them as a separate program, so the GPL-2.0-only plugins do not combine with vhs-gui's GPL-3.0.
      The `.deb` contains none of the wheels.
- [x] First step (2026-11-07): `scripts/build-vs-plugins.sh` builds the three plugins from pinned
      source (`tools/vs-plugins/Dockerfile`: ffms2 5.0, fmtconv r30, miscfilters R2, headers
      R81) in a container and loads them under R81: all three load, and ffms2 decodes, fmtc
      resamples, misc lists its filters. Findings: (1) ffms2 links FFmpeg's shared libraries, so a
      24.04 build (libavcodec.so.60) does not load on 26.04 (so.62): build one per target
      release, or bundle ffmpeg libs; (2) the tag `ffms-2.17` is an old line, 5.0 is current;
      (3) miscfilters has no build system, so it is one `g++` call, and it needs
      `filtershared.h` from R73 (R81's copy uses an API 4.3 call); (4) fmtconv is 4.7 MB stripped.
- [x] **QTGMC on a real raw capture under R81** (2026-11-15). A 2 h 30 min capture (720x480, 30 fps,
      denoised) went through Denoise+QTGMC from the app with `vspipe` from the private environment
      that "Set up VapourSynth" built (R81, the pinned wheels, Ryan's own havsfunc and plugin
      folders): 270,581 frames in 53 min 41 s (84 fps, 2.8x real time), 9019.366 s against 9019.373 s
      for the source, no errors in the log, tail and middle decode clean, and frames sampled at five
      points (10 min to 2 h 30 min) show the same picture without combing. Not checked: a frame-by-frame
      comparison with R55's output, and the pinned downloaded havsfunc (the run used Ryan's vsrepo copy,
      which comes first on `PYTHONPATH`; the pinned one was run in a container).
- [ ] Optional later: AppImage (linuxdeploy + appimagetool) with `libmpv.so.2`, ffmpeg, ffprobe and
      sox bundled (GPL: ship a sources note), `scripts/package-appimage.sh`, attach to the release,
      test with `/usr/bin/ffmpeg` hidden; Wayland and GL under AppImage can need quirks.
