# Embedded copy of vhs-cli/vhs-env/tools/ivtc_decombed.vpy. Two differences: plugins are found
# through vhs_plugins.py instead of absolute /home paths, and the FrameEval callback reads the
# pre-FrameEval clip (`base`). The original's lambda read `clip`, which by then is the FrameEval
# clip itself, so any frame VFM did not flag as combed requested a frame of itself and vspipe hung.
import os
import vapoursynth as vs
import vhs_plugins
import havsfunc as haf

core = vs.core

# Explicit plugin loads (since autoload has been unreliable)
vhs_plugins.load("libvivtc.so")
vhs_plugins.load("libffms2.so")
vhs_plugins.load("libmvtools.so")
vhs_plugins.load("libnnedi3.so")
vhs_plugins.load("libmiscfilters.so")

SRC = os.environ["VS_INPUT"]
TFF = int(os.environ.get("VS_TFF", "1"))

# QTGMC preset for combed-frame cleanup. Lighter = faster but less clean.
# "Fast" is a good balance for post-IVTC decombing.
DECOMB_PRESET = os.environ.get("VS_DECOMB_PRESET", "Fast")

clip = core.ffms2.Source(source=SRC)

# VFM: field matching with micmatch=2 (more aggressive matching to reduce combing)
# micout=1 outputs a _Combed frame property indicating combed frames
clip = core.vivtc.VFM(clip, order=TFF, micmatch=2, micout=1)

# Prepare QTGMC deinterlaced version for combed frame replacement
# FPSDivisor=2 keeps original framerate (not double)
qtgmc = haf.QTGMC(clip, Preset=DECOMB_PRESET, TFF=TFF, FPSDivisor=2, EdiMode="nnedi3")

# Replace only combed frames with QTGMC output, leave clean frames untouched
# VFM sets _Combed=1 on frames it couldn't cleanly match
base = clip
clip = core.std.FrameEval(
    base,
    lambda n, f: qtgmc if f.props.get("_Combed", 0) == 1 else base,
    prop_src=base,
)

# VDecimate: removes the duplicate frame from each 5-frame telecine group (30fps → 24fps)
clip = core.vivtc.VDecimate(clip)

clip.set_output()
