"""Plugin loader shared by the embedded vhs-gui VapourSynth scripts.

VapourSynth autoload has been unreliable here, so each script loads the plugins it needs
explicitly. Plugins are looked up by file name in a list of directories instead of by absolute
path, so the scripts work on any account. Set VS_PLUGIN_DIRS (colon-separated) to override.
"""

import os

import vapoursynth as vs

_DEFAULT_DIRS = (
    "~/.local/share/vsrepo/plugins",
    "/usr/lib/x86_64-linux-gnu/vapoursynth",
    "/usr/lib/vapoursynth",
    "/usr/local/lib/vapoursynth",
)


def plugin_dirs():
    override = os.environ.get("VS_PLUGIN_DIRS")
    if override:
        return [d for d in override.split(":") if d]
    return [os.path.expanduser(d) for d in _DEFAULT_DIRS]


def load(filename):
    """Load plugin `filename` (e.g. "libffms2.so") from the first directory that has it."""
    for directory in plugin_dirs():
        path = os.path.join(directory, filename)
        if not os.path.isfile(path):
            continue
        try:
            vs.core.std.LoadPlugin(path=path)
        except vs.Error as e:
            if "already loaded" not in str(e):
                raise
        return path
    raise FileNotFoundError(
        "VapourSynth plugin %s not found in: %s" % (filename, ", ".join(plugin_dirs()))
    )
