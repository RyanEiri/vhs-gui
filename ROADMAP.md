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

- [ ] Port `vhs_viewer_encode.sh` to a native ffmpeg job (H.264/AAC MKV, CRF 18 default).
- [ ] Remove `viewer_encode_script()`.

## Phase 3: Denoise and Denoise+QTGMC

- [ ] Port `vhs_denoise.sh` and `denoise.sh` as a `SeqStep` sequence reusing `audio_cleanup.rs`
      (video bit-exact copy, PCM audio).
- [ ] Port `vhs_process.sh`: denoise, idet sample, QTGMC through `vs_ops`. `FORCE_QTGMC`,
      `SKIP_QTGMC` and `REDO_*` become GUI options.
- [ ] Remove `denoise_script()` and `process_script()`.

## Phase 4: capture

- [ ] Compile the `capture.env` defaults into a Rust struct (file stays an optional override).
- [ ] Spawn ffmpeg directly and hold the `Child`; keep the re-armable Cap timer, the 6 h `-t`
      backstop, `seg###.mkv` naming and Ctrl+C as a normal stop.
- [ ] Remove `capture_script()`. Test with a v4l2 loopback or a short real capture.

## Phase 5: upscale (largest, riskiest)

- [ ] One native engine for all three variants (variant = model, plus `hue=s=0` for B&W): segment
      loop, frame extract, Real-ESRGAN (ROCm `BATCH_SIZE=2` or ncnn), downscale, H.264 encode, concat,
      config fingerprint (`ALLOW_MIXED`), resumable `seg_*.mp4`, `run_config.txt`.
- [ ] Rust writes the lock (`upscale.pgid`) itself. `check_lock` accepts a vhs-gui worker as owner
      and keeps accepting old `vhs_upscale*.sh` jobs for one release. Update the CLAUDE.md rule that
      Rust only reads PGID files.
- [ ] Decide whether the engine runs as a detached `vhs-gui --upscale-worker` so a job still
      outlives the GUI. Tests: fingerprint, resume after kill, stop after segment.
- [ ] Keep the bash path selectable until one real tape upscale has matched.
- [ ] Remove the three `upscale*_script()` accessors, `scripts_dir`, the "Scripts dir" field, and the
      "Transitional bash dependency" section of CLAUDE.md.

## Phase 6: portable package

- [ ] AppImage (linuxdeploy + appimagetool): binary, `libmpv.so.2` and its libraries, ffmpeg,
      ffprobe and sox with their libraries, icon and `.desktop`. `tools.rs` prefers the bundled dir.
- [ ] `scripts/package-appimage.sh` (reproducible), README install section, licenses file (the
      ffmpeg and sox builds are GPL: ship a sources note).
- [ ] Attach the AppImage to the GitHub release from the next tag.
- [ ] Test on the real desktop with `/usr/bin/ffmpeg` hidden (clean container or stripped `PATH`);
      Wayland and GL under AppImage can need quirks.
- [ ] Optional: a `.deb` through `cargo-deb` with `Depends:` instead of bundling.
