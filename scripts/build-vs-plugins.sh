#!/usr/bin/env bash
# Build ffms2, fmtconv and miscfilters from pinned source in a container and prove they
# load under VapourSynth R81 (in a container when the release is not this machine's).
# Usage: scripts/build-vs-plugins.sh [ubuntu-release] [r81-venv]
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

check='
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
'

host_release="$(. /etc/os-release && echo "$VERSION_ID")"
if [[ "$release" == "$host_release" && -x "$venv/bin/python" ]]; then
    cd "$out"
    "$venv/bin/python" -c "$check"
else
    # Another release's plugins link that release's FFmpeg, so they cannot load here: check them in
    # a container of that release, with VapourSynth R81 installed from the pinned wheel.
    docker run --rm -e CHECK="$check" -v "$here/assets/vs/requirements.txt:/req.txt:ro" "$image" sh -ec '
        apt-get update -qq >/dev/null
        apt-get install -y -qq python3-venv "libpython3.$(python3 -c "import sys; print(sys.version_info[1])")" >/dev/null 2>&1
        python3 -m venv /v
        grep -A1 "^VapourSynth==" /req.txt > /tmp/vs.txt
        /v/bin/python -m pip install -q --require-hashes --only-binary=:all: --no-deps --disable-pip-version-check -r /tmp/vs.txt
        HOME=/root /v/bin/vapoursynth config >/dev/null
        cd /out && HOME=/root /v/bin/python -c "$CHECK"
    '
fi
