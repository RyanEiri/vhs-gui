# Embedded copy of vhs-cli/vhs-env/tools/qtgmc.vpy. Only the plugin loading differs:
# plugins are found through vhs_plugins.py instead of absolute /home paths.
import os
import vapoursynth as vs
import vhs_plugins
import havsfunc as haf

core = vs.core

# Explicit plugin loads (since autoload has been unreliable)
vhs_plugins.load("libffms2.so")
vhs_plugins.load("libmvtools.so")
vhs_plugins.load("libfmtconv.so")
vhs_plugins.load("libnnedi3.so")
vhs_plugins.load("libmiscfilters.so")
# R81 has znedi3 instead of nnedi3; havsfunc is told to call it (no effect on R55)
vhs_plugins.prepare_havsfunc(haf)

SRC = os.environ["VS_INPUT"]
TFF = int(os.environ.get("VS_TFF", "1"))
FPSDIV = int(os.environ.get("VS_FPSDIV", "2"))
PRESET = os.environ.get("VS_PRESET", "Slower")

clip = core.ffms2.Source(source=SRC)

# No resize/format conversion here (your input is already yuv422p 8-bit)
clip = haf.QTGMC(
    clip,
    Preset=PRESET,
    TFF=TFF,
    FPSDivisor=FPSDIV,
    EdiMode="nnedi3",
)

clip.set_output()

