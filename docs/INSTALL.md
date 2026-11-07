# Installing the external tools

vhs-gui does not bundle the programs it drives. This page lists each one, how to get it, and how
to check it. The **Tools** window (🔧 in the left rail) shows what vhs-gui found, with paths and
versions, and the toolbar warns when something is missing.

Everything here was checked on Ubuntu 26.04, amd64, with an AMD RX 7800 XT. Steps marked
*(not tested)* are what upstream documents but were not run for this page. Other distributions
need the same programs with different install commands.

| Needed for | Program | Section |
|---|---|---|
| everything | ffmpeg, ffprobe | [1](#1-system-packages) |
| audio cleanup | sox | [1](#1-system-packages) |
| live preview | libmpv | [1](#1-system-packages) |
| QTGMC, IVTC, VDecimate, field align | VapourSynth, plugins, Python modules | [2](#2-vapoursynth) |
| AI upscale (Vulkan) | realesrgan-ncnn-vulkan | [3](#3-real-esrgan-vulkan-ncnn) |
| AI upscale (ROCm, community VHS models) | PyTorch + ROCm driver | [4](#4-real-esrgan-rocm-pytorch) |
| both upscalers | model weights | [5](#5-model-weights) |

## 1. System packages

```bash
sudo apt install ffmpeg sox libmpv2
```

In use here: ffmpeg 8.0.1 (the distro build, `/usr/bin/ffmpeg`), SoX 14.7, libmpv2 0.41.
vhs-gui looks in `/usr/bin` before `PATH`, so a different ffmpeg elsewhere (for example a
DeckLink build in `/usr/local/bin`) is not picked up. To use another binary, set
`VHS_GUI_FFMPEG`, `VHS_GUI_FFPROBE` or `VHS_GUI_SOX`.

**Building from source** also needs a `libmpv.so` link for the linker. On the author's machine it
is a hand-made symlink to `libmpv.so.2` that no package owns. `libmpv-dev` is the usual way to get
it *(not tested)*. See the README for `cargo build`.

## 2. VapourSynth

vhs-gui runs its VapourSynth scripts itself (they are embedded in the program). What it needs on
the machine is VapourSynth, a set of plugins, and a set of Python modules.

### 2a. VapourSynth R55 from a PPA

```bash
sudo add-apt-repository ppa:savoury1/vapoursynth
sudo apt update
sudo apt install vapoursynth vapoursynth-python3 vspipe
```

This is the source of the R55 build in use (`55~api3+ds0-0ubuntu1~24.04.sav3`, from
`ppa.launchpadcontent.net/savoury1/vapoursynth`). The PPA package is built for Ubuntu 24.04 and
Python **3.12**; `vspipe` embeds Python 3.12 even though the system Python is 3.14.

**Keep the Python 3.12 libraries installed.** Ubuntu 26.04 has no `python3.12` interpreter, only
the shared libraries `vspipe` loads, and they are marked automatically installed, so
`apt autoremove` would remove them and break `vspipe`. Mark them as kept:

```bash
sudo apt-mark manual libpython3.12t64 libpython3.12-stdlib libpython3.12-minimal \
    vapoursynth-python3 vspipe
```

Check: `vspipe --version` prints `Core R55`.

### 2b. Plugins and Python modules

The scripts load these plugins (from `~/.local/share/vsrepo/plugins`, then the system VapourSynth
directories; `VS_PLUGIN_DIRS` overrides): `libffms2.so`, `libvivtc.so`, `libmvtools.so`,
`libfmtconv.so`, `libnnedi3.so` with `nnedi3_weights.bin` beside it, and `libmiscfilters.so`.
`libvivtc.so` comes with the VapourSynth package (`/usr/lib/x86_64-linux-gnu/vapoursynth`); the
others were installed with [vsrepo](https://github.com/vapoursynth/vsrepo).

`havsfunc` (QTGMC) needs two Python modules in `~/.local/share/vsrepo/py`: `havsfunc` and `vsutil`.
vhs-gui puts that directory on `PYTHONPATH` for `vspipe`. (vsrepo also installs `mvsfunc`, `adjust` and
`nnedi3_resample` because havsfunc declares them, but havsfunc only imports them inside functions that
vhs-gui does not use, so they are not required.) Both were installed by vsrepo.

vsrepo's package names differ from the file names: `libmiscfilters.so` is the package **`misc`**
("Miscfilters (obsolete)"), `libffms2.so` is `ffms2`, and so on. These names exist in vsrepo's
package list. With vsrepo, the layout above is what these switches produce *(command not run on
this machine, see the gap below)*:

```bash
git clone https://github.com/vapoursynth/vsrepo ~/opt/vsrepo
python3 ~/opt/vsrepo/vsrepo.py -b ~/.local/share/vsrepo/plugins -s ~/.local/share/vsrepo/py \
    install havsfunc vsutil ffms2 mvtools fmtconv nnedi3 misc
```

`havsfunc` declares `adjust`, `mvsfunc`, `nnedi3_resample`, `mvtools` and `misc` as dependencies,
so vsrepo installs them with it.

> **Known gap: a fresh install on Ubuntu 26.04.** vsrepo only runs under a Python that can
> `import vapoursynth`. The R55 module is built for Python 3.12 and 26.04 has no 3.12 interpreter,
> so on this machine `vsrepo.py` stops with "Could not detect vapoursynth" (checked 2026-11-02).
> The plugins and modules here were installed before the upgrade to 26.04. Until the VapourSynth
> version question is settled (R55 on Python 3.12, or a newer release on Python 3.14; see the
> roadmap), there is no tested way to install them from scratch. The Tools window will list exactly
> which ones are missing.
>
> **Version drift to expect.** vsrepo's current `havsfunc` entry lists the `znedi3` plugin as its
> nnedi3 dependency, while the `havsfunc.py` installed here (December 2025) calls `core.nnedi3`
> from `libnnedi3.so`. A fresh install today may therefore fetch a newer `havsfunc` that expects
> `libznedi3.so`, which the embedded scripts do not load. This was read from vsrepo's package files
> and not tried. If QTGMC fails after a fresh install, look here first.

## 3. Real-ESRGAN, Vulkan (ncnn)

Download the Ubuntu release from
[xinntao/Real-ESRGAN-ncnn-vulkan](https://github.com/xinntao/Real-ESRGAN-ncnn-vulkan/releases/tag/v0.2.0)
(asset `realesrgan-ncnn-vulkan-v0.2.0-ubuntu.zip`), then:

```bash
mkdir -p ~/opt/realesrgan-ncnn ~/bin
unzip realesrgan-ncnn-vulkan-v0.2.0-ubuntu.zip -d /tmp/ncnn
find /tmp/ncnn -name realesrgan-ncnn-vulkan -type f -exec install -m 0755 {} ~/opt/realesrgan-ncnn/ \;
ln -sf ~/opt/realesrgan-ncnn/realesrgan-ncnn-vulkan ~/bin/realesrgan-ncnn-vulkan
```

*(not tested: the `find` step is there because the zip's layout was not checked.)* Needs a working Vulkan driver (`vulkaninfo --summary`).
The Vulkan backend only offers the model families upstream hard-codes: `realesrgan-x4plus`,
`realesrgan-x4plus-anime`, `realesrnet-x4plus`, `realesr-animevideov3*`. The community VHS models
need the ROCm backend.

## 4. Real-ESRGAN, ROCm (PyTorch)

The ROCm backend is the driver in [`tools/realesrgan-rocm/`](../tools/realesrgan-rocm/):
`driver.py` (a `realesrgan-ncnn-vulkan` drop-in that runs `.pth` models through
[spandrel](https://github.com/chaiNNer-org/spandrel)), `requirements.txt`, and the
`realesrgan-rocm` shim. The shim expects this layout, which is also what vhs-gui's upscale scripts
call:

```
~/opt/realesrgan-rocm/{driver.py,requirements.txt,venv/,models/}
~/bin/realesrgan-rocm
```

Requirements: an AMD GPU with the `amdgpu` kernel driver, and your user in the `render` and
`video` groups (`/dev/kfd` is `root:render`, mode 660). The PyTorch ROCm wheels bring their own
ROCm user-space libraries; no separate ROCm install was needed here.

```bash
mkdir -p ~/opt/realesrgan-rocm/models ~/bin
cp tools/realesrgan-rocm/{driver.py,requirements.txt} ~/opt/realesrgan-rocm/
install -m 0755 tools/realesrgan-rocm/realesrgan-rocm ~/bin/realesrgan-rocm
python3 -m venv ~/opt/realesrgan-rocm/venv
~/opt/realesrgan-rocm/venv/bin/pip install torch torchvision \
    --index-url https://download.pytorch.org/whl/rocm7.1
~/opt/realesrgan-rocm/venv/bin/pip install -r ~/opt/realesrgan-rocm/requirements.txt
```

The package set above is what is installed here (Python 3.14.4, torch 2.13.0+rocm7.1); the
`rocm7.1` wheel index was confirmed to exist, but the venv was not rebuilt from scratch for this
page. The first run after a rebuild spends a couple of minutes compiling HIP kernels. The shim
sets `HSA_OVERRIDE_GFX_VERSION=11.0.0` because the RX 7800 XT (gfx1101) is not in the prebuilt
wheel's target list; export your own value to override it, and expect to need one for other GPUs.
Use `BATCH_SIZE=2` for the x4plus models on a 16 GB card.

Check: `~/bin/realesrgan-rocm -h` prints the usage.

## 5. Model weights

Weights are not in this repository. The official and community VHS-tuned models are attached to
the `models-v1` release of vhs-cli:
<https://github.com/RyanEiri/vhs-cli/releases/tag/models-v1>

| Archive | Size | Extract so the files sit directly in |
|---|---|---|
| `realesrgan-models-ncnn.zip` | 228 MB | `~/opt/realesrgan-ncnn/models/` (`.param` + `.bin`) |
| `realesrgan-models-pytorch.tar.gz` | 326 MB | `~/opt/realesrgan-rocm/models/` (`.pth`) |

*(not tested: the layout inside each archive.)* Each model also needs a `MODEL_MAP` entry in
`driver.py` for the ROCm backend; the shipped map covers `realesrgan-x4plus`,
`realesrgan-x4plus-anime`, `realesrgan-x2plus`, `2x_VHS-Film`, `ToonVHS-1x` and `VHS-Sharpen-1x`.

## Checking it all

Open the 🔧 Tools window. Every row should show ✓. A ✓ means the file exists where vhs-gui
looks; it does not prove the program runs, so the first real job is still the final test.
