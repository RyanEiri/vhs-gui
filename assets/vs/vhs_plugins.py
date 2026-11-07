"""Plugin loader shared by the embedded vhs-gui VapourSynth scripts.

VapourSynth autoload has been unreliable here, so each script loads the plugins it needs
explicitly. Plugins are looked up by file name in a list of directories instead of by absolute
path, so the scripts work on any account. Set VS_PLUGIN_DIRS (colon-separated) to override.

R81 has no API 3 support: the vsrepo `libmvtools.so` and `libnnedi3.so` cannot be loaded by it, and
its own API 4 builds (the `vapoursynth-mvtools` and `vapoursynth-znedi3` wheels) are found by its
autoloader in its own plugin folder. So `load` first checks whether VapourSynth already provides the
namespace a file stands for, and the folder returned by `vapoursynth.get_plugin_dir()` (absent in R55)
is searched as well.
"""

import os

import vapoursynth as vs

_DEFAULT_DIRS = (
    "~/.local/share/vsrepo/plugins",
    "/usr/lib/x86_64-linux-gnu/vapoursynth",
    "/usr/lib/vapoursynth",
    "/usr/local/lib/vapoursynth",
)


# Plugin namespaces that make loading a file unnecessary (the core already has an equivalent).
_PROVIDED_BY = {
    "libmvtools.so": ("mv",),
    "libnnedi3.so": ("nnedi3", "znedi3"),
    "libvivtc.so": ("vivtc",),
}


def plugin_dirs():
    override = os.environ.get("VS_PLUGIN_DIRS")
    if override:
        return [d for d in override.split(":") if d]
    dirs = [os.path.expanduser(d) for d in _DEFAULT_DIRS]
    own = getattr(vs, "get_plugin_dir", None)
    if own is not None:
        path = own()
        if path and path not in dirs:
            dirs.append(path)
    return dirs


def _already_provided(filename):
    return any(hasattr(vs.core, ns) for ns in _PROVIDED_BY.get(filename, ()))


def load(filename):
    """Load plugin `filename` (e.g. "libffms2.so") from the first directory that has it.

    Returns the path loaded, or None if VapourSynth already provides an equivalent plugin."""
    if _already_provided(filename):
        return None
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
