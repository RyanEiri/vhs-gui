# Embedded copy of vhs-cli/vhs-env/tools/ivtc.vpy. Only the plugin loading differs:
# plugins are found through vhs_plugins.py instead of absolute /home paths.
import os
import vapoursynth as vs
import vhs_plugins

core = vs.core

# Explicit plugin loads (since autoload has been unreliable)
vhs_plugins.load("libvivtc.so")
vhs_plugins.load("libffms2.so")

SRC = os.environ["VS_INPUT"]
TFF = int(os.environ.get("VS_TFF", "1"))

clip = core.ffms2.Source(source=SRC)

# VFM: field matching — reconstructs progressive frames from interlaced fields
clip = core.vivtc.VFM(clip, order=TFF)

# VDecimate: removes the duplicate frame from each 5-frame telecine group (30fps → 24fps)
clip = core.vivtc.VDecimate(clip)

clip.set_output()
