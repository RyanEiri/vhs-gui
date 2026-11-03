# Embedded copy of vhs-cli/vhs-env/tools/vdecimate.vpy. Only the plugin loading differs:
# plugins are found through vhs_plugins.py instead of absolute /home paths.
import os
import vapoursynth as vs
import vhs_plugins

core = vs.core

# Explicit plugin loads (since autoload has been unreliable)
vhs_plugins.load("libvivtc.so")
vhs_plugins.load("libffms2.so")

SRC = os.environ["VS_INPUT"]

clip = core.ffms2.Source(source=SRC)

# VDecimate: removes duplicate frames from progressive content.
# Used after QTGMC to drop duplicates re-introduced during deinterlacing
# of telecined material (30fps → 24fps).
clip = core.vivtc.VDecimate(clip)

clip.set_output()
