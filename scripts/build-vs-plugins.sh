#!/usr/bin/env bash
# Build ffms2, fmtconv and miscfilters from pinned source in a container and prove they
# load under VapourSynth R81. Usage: scripts/build-vs-plugins.sh [ubuntu-release] [r81-venv]
# The plugins link FFmpeg dynamically, so build for the release they will run on.
set -euo pipefail
release="${1:-26.04}"
venv="${2:-$HOME/venv/vapoursynth-r81}"
here="$(cd "$(dirname "$0")/.." && pwd)"
out="${XDG_CACHE_HOME:-$HOME/.cache}/vhs-gui-vs-plugins/$release"
image="vhs-gui-vs-plugins:$release"

docker build --build-arg "UBUNTU=$release" -t "$image" "$here/tools/vs-plugins"
mkdir -p "$out"
cid="$(docker create "$image")"
trap 'docker rm "$cid" >/dev/null' EXIT
docker cp "$cid:/out/." "$out/"
strip --strip-unneeded "$out"/*.so* 2>/dev/null || true
cat "$out/REVISIONS"

cd "$out"
"$venv/bin/python" - <<'PY'
import os, sys, vapoursynth as vs
bad = 0
for name, ns in (("libffms2.so", "ffms2"), ("libfmtconv.so", "fmtc"), ("libmiscfilters.so", "misc")):
    try:
        vs.core.std.LoadPlugin(os.path.abspath(name))
        assert hasattr(vs.core, ns)
        print("OK  ", name)
    except Exception as e:
        bad += 1
        print("FAIL", name, e)
sys.exit(bad)
PY
