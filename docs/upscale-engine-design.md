# Native upscale engine: design (Phase 5)

Status: decisions agreed (2026-11-03); sub-phase 5a built and tested (see section 6). Written from reading `vhs_upscale.sh`,
`vhs_upscale_anime.sh`, `vhs_upscale_bw.sh` (599 + 696 + 515 lines), `src/pipeline.rs`,
`src/settings.rs` and `src/panels/upscale.rs`.

## 1. What the scripts do today

All three run one pipeline. For input `IN`, output `OUT`, segment length `S` (GUI default 30 s)
and CRF (21):

1. Probe duration, `r_frame_rate`, width and height with ffprobe. Output size is
   `FINAL_H = src_h * FINAL_SCALE`, `FINAL_W = FINAL_H * 4 / 3` rounded up to even (`TARGET_DAR` 4:3).
2. Write `run_config.txt` (the "fingerprint") in `WORK_ROOT/<input stem>/`. If segments already exist
   and the fingerprint differs the run stops, unless `ALLOW_MIXED=1`.
3. Write `WORK_DIR/upscale.pgid` (removed on exit). A later GUI process reads it to reattach.
4. For each segment `i` (skip it if `segments/seg_NNN.mp4` is non-empty):
   - ffmpeg: `-ss start -t len -i IN -an [-vf PRE_VF] -qscale:v 2 frames/frame_%08d.jpg`
   - upscaler: `realesrgan-rocm|realesrgan-ncnn-vulkan -i frames -o frames_up -s INTERNAL -m MODELS
     -n MODEL -t TILE -j THREADS -g VK -f jpg`, then check the frame counts match
   - ffmpeg: `-framerate fps -i frames_up/frame_%08d.jpg -vf scale=W:H:flags=lanczos,setsar=1 -an
     -c:v libx264 -preset veryfast -crf CRF -pix_fmt yuv420p seg_NNN.mp4`, then check the packet count
   - delete `frames/` and `frames_up/`
5. Validate the segments (readable, none under S/3 except the last, total duration within 2% of the
   source). On failure delete the bad ones and run the loop once more, then validate again.
6. `ffmpeg -f concat -c copy`, then mux the source's first audio stream as AAC 160k (or copy the
   video if there is no audio).
7. Run `vhs_fix_sync.sh` on the result (the Rust port of that already exists: `fix_sync.rs`).

The GUI passes `CRUSH`, `BRIGHTNESS`, `UPSCALE_BACKEND`, `INTERNAL_SCALE`, `FINAL_SCALE`, `MODEL`,
`MODELS_DIR` (Vulkan) or `BATCH_SIZE` (ROCm), `WORK_ROOT`, and the positional arguments
`OUT SEGMENT_SECONDS CRF`.

### The variants differ very little

| | standard | anime | B&W |
|---|---|---|---|
| default model | `realesrgan-x4plus` | `realesrgan-x4plus-anime` | `realesrgan-x4plus` |
| pre-filter | `hqdn3d` + optional crush/brightness | same | same, with `hue=s=0` in every preset; env var is `BW_FILTER` not `PRE_VF` |
| frame extraction `-vf` | `PRE_VF` | `PRE_VF` | `BW_FILTER,format=rgb24` |
| rebuild `-vf` | `scale,setsar=1` | same | `scale,setsar=1,format=yuv420p` |
| drift tolerance | 2% | **8%** | 2% |
| `SCRIPT=` in the fingerprint | `vhs_upscale.sh` | `vhs_upscale_anime.sh` | `vhs_upscale_bw.sh` |
| optional `DECOMB` (per-segment IVTC + QTGMC) | no | yes | no |

### Findings that shape the port

- **`DENOISE=1` is dead.** The GUI exports it when `settings.denoise` is set (there is a "Denoise"
  checkbox in the upscale settings panel), but none of the three scripts read it and neither does the
  ROCm driver (it reads `ROCM_DTYPE`, `BATCH_SIZE` and `HIP_VISIBLE_DEVICES`). Drop it. (An earlier
  draft of this document wrongly said nothing in the UI sets it.)
- **`DECOMB` is unreachable from the GUI.** The anime script supports it; the GUI never passes it.
  Not ported now; it can come back as an option later (the embedded `ivtc_decombed.vpy` is there).
- **A killed segment encode can poison a resume.** The script trusts any non-empty `seg_NNN.mp4`. If
  ffmpeg is interrupted mid-encode the partial file stays and is skipped next time; only the
  "under S/3 seconds" check can catch it. The port should write `seg_NNN.mp4.part` and rename it
  after the frame-count check (the GUI counts `seg_*.mp4`, which a `.part` file does not match).
- The scripts hard-code `/usr/bin/ffmpeg`, so they ignore the tool resolver and `VHS_GUI_FFMPEG`.

## 2. Decisions for Ryan

1. **Where the engine runs.** Today a job outlives the GUI (multi-hour GPU work, `on_exit` does not
   cancel it, a later GUI reattaches through `upscale.pgid`). Recommended: a **detached worker
   process**, the same binary run as `vhs-gui upscale-worker <job file>`, in its own process group.
   That keeps all of: outlives the GUI, reattach, pause/resume (SIGSTOP/SIGCONT to the group), stop
   after segment and cancel (SIGINT). The alternative, engine threads inside the GUI, is simpler but
   a GUI crash would kill a multi-hour job; this machine has had several GUI crashes.
2. **Faithful port plus two deliberate fixes:** the `.part` rename above, and tool paths through the
   resolver. Everything else (commands, defaults, fingerprint text, validation limits) identical.
3. **Rollout.** Keep the bash scripts selectable behind a setting ("Upscale engine: native / script"),
   default **script**, until one real upscale matches. Flip the default, then delete the scripts.
4. **Scope.** No `DECOMB`, no `DENOISE`.

## 3. Design

### Modules (`src/upscale/`)
- `plan.rs`: pure functions. Probe results to segment plan, output size, pre-filter chains (CRUSH,
  BRIGHTNESS, B&W), the exact command arguments for the three external steps, and the
  `run_config.txt` text. Everything the golden tests compare is here.
- `engine.rs`: the segment loop, frame-count checks, validation and one auto-repair pass, concat,
  mux, sync fix. Runs the steps through `tools::command`.
- `worker.rs`: the `upscale-worker` entry point: read the job file, write `upscale.pgid` (removed on
  exit), handle SIGINT, run the engine, exit code 0 / non-zero.
- `variant.rs`: `Variant { Standard, Anime, Bw }` and the per-variant defaults in the table above.
- `main.rs` checks for the `upscale-worker` argument before any GUI or GL setup (running the binary
  with other arguments must keep opening the GUI exactly as now).

### Compatibility with what the GUI already does
- Work directory layout unchanged: `WORK_ROOT/<stem>/{segments,frames,frames_up,run_config.txt,upscale.pgid}`.
  The GUI's progress tracking (counting `seg_*.mp4`, frames in the two directories) needs no change.
- `run_config.txt` is produced byte for byte as the script does (same keys, same order, `SCRIPT=`
  naming the old script, `PRE_VF=` or `BW_FILTER=`), so a work directory started by a script can be
  resumed natively and the fingerprint guard still works.
- `is_upscale_group_leader` and `check_lock` accept a `vhs-gui upscale-worker` command line as well
  as `vhs_upscale*.sh`, for one release, so jobs started by either can be reattached.
- Signals: SIGSTOP/SIGCONT to the group pause and resume. SIGINT sets a flag in the worker, which
  also reaches ffmpeg and the upscaler in the group; the worker then removes any `.part` file and
  exits non-zero without starting another segment. "Stop after segment" keeps working because the
  GUI sends it between segments, as now.
- Sync fix: `fix_sync` gets a synchronous entry point (its pure parts already exist) for the last step.

### Sub-phases
- **5a** engine core, worker, standard variant, `.part` handling, settings toggle, golden and
  equivalence tests. Script path stays the default.
- **5b** anime and B&W variants; all six upscale buttons wired.
- **5c** lock and reattach compatibility, stale-work-dir handling, resume and cancel tests.
- **5d** after a real upscale matches: flip the default, remove the three scripts' accessors,
  `scripts_dir`, the "Scripts dir" field, `PipelineJob::start` and the bash call site, and the
  "Transitional bash dependency" section.

## 4. Testing

- **Golden (no GPU):** run a copy of the real script, with only the two hard-coded binary paths
  replaced, against stub ffmpeg/ffprobe/upscaler programs that log their arguments and create the
  files the script expects, for each variant and for a short and a multi-segment input. The Rust
  engine, run against the same stubs, must produce the same command sequence and the same
  `run_config.txt`. (The same approach caught real differences in the Denoise and capture ports.)
- **Equivalence (GPU, ignored test):** a short real clip through the script and through the engine
  with the ROCm backend; compare frame counts and duration, and the output hash if it is
  deterministic (x264 and JPEG extraction should be; the ROCm batches may not be).
- **Behaviour:** resume after a kill (segments skipped, fingerprint guard refuses a changed config,
  `ALLOW_MIXED`), a killed encode leaves no `seg_*.mp4`, frame-count mismatch fails loudly, a short
  segment triggers the auto-repair pass, pause/resume/stop-after-segment/cancel on a stub upscaler,
  reattach to a running worker.
- Safety rule from the capture work: tests that run a real script neutralize every file it sources
  and every hardware path, run with a temp `HOME`, a time limit and their own process group, and
  assert the stubs were called.

## 5. Risks

- A real upscale takes hours, so the final "matches a real tape" check is slow; a 5 minute clip
  covers the equivalence question.
- GPU memory and driver behaviour are outside what the stubs can show; the ROCm check needs the
  GPU free (no other upscale running).
- Reattach and signal handling are subtle (a pid-reuse bug was already fixed once in `check_lock`);
  they get their own tests before the default flips.

## 6. Status

**5a done (2026-11-03):** `src/upscale/{plan,engine,worker}.rs`, the `upscale-worker` mode in `main`,
the Script/Native engine setting (default Script), and native launch for the plain Upscale buttons.
**5b done (2026-11-03):** Anime and B&W are wired to the native engine. The golden tests
`the_anime_engine_matches_vhs_upscale_anime_sh` and `the_bw_engine_matches_vhs_upscale_bw_sh` compare
calls and `run_config.txt` with the real scripts (a mutation of the B&W filter makes the test fail).
Not yet run on the GPU: a real-clip equivalence for these two variants.

Evidence:
- 38 unit tests on the pure planning code and the engine against stub tools (full run, resume, the
  fingerprint guard, both frame-count checks, cancel mid-segment, auto-repair and its failure path,
  no audio, preflight errors, ROCm batch size).
- Golden test (`cargo test -- --ignored`): the real `vhs_upscale.sh`, run with stubs, and the engine
  make the same ffprobe, ffmpeg and upscaler calls and write the same `run_config.txt` for six
  configurations (three segments, one segment, no audio, ROCm with a community model, heavy crush with
  brightness, PAL). Breaking one flag on purpose made it fail.
- Worker integration tests with the real binary: lock file and process group, pause and resume with
  SIGSTOP/SIGCONT, SIGINT cancel (exit 130, no checkpoint), a second worker refused, recovery after
  SIGKILL, usage errors.
- GPU equivalence test (`cargo test --test upscale_equivalence -- --ignored`): the script and the
  worker on an 8 s clip with the ROCm backend and the real tools produced the same streams, 239
  frames, 8.008 s, and **bit-identical video frames and audio**.

Not yet verified: a real tape-length upscale (gates the default flip, 5d); the GUI toggle by hand.
