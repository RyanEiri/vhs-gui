#!/usr/bin/env bash
# Build a .deb of vhs-gui for the Ubuntu release it runs on.
#
#   scripts/package-deb.sh [--container] [ubuntu-release]   # default: this machine's VERSION_ID
#
# A release other than this machine's, or --container, builds inside a clean container of that
# release (packaging/Dockerfile.build, needs docker): the binary then links that release's glibc
# and libmpv. Build caches are kept in ~/.cache/vhs-gui-deb-build/<release>/.
#
# Needs: cargo, dpkg-deb, dpkg-shlibdeps (dpkg-dev), lintian (optional), and the three VapourSynth
# plugins that have no wheel, built for that release in ~/.cache/vhs-gui-vs-plugins/<release>/ by
# scripts/build-vs-plugins.sh (this script runs it, which needs docker, when they are missing).
# The plugins link FFmpeg's shared libraries, so a package is only good for the release it was
# built for. Output: dist/vhs-gui_<version>-1~ubuntu<release>_amd64.deb. Nothing is installed.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

container=0
if [[ "${1:-}" == "--container" ]]; then
    container=1
    shift
fi
host_release="$(. /etc/os-release && echo "$VERSION_ID")"
release="${1:-$host_release}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-1~ubuntu$release"
cache="${XDG_CACHE_HOME:-$HOME/.cache}"
plugins="$cache/vhs-gui-vs-plugins/$release"
out="$here/dist"
pkg="vhs-gui_${version}_amd64"
target="${CARGO_TARGET_DIR:-target}"

if [[ ! -e "$plugins/libffms2.so" || ! -e "$plugins/libfmtconv.so" || ! -e "$plugins/libmiscfilters.so" ]]; then
    echo ">>> plugins for $release not built yet"
    scripts/build-vs-plugins.sh "$release"
fi

# Re-run this script inside a container of the target release (the inner run sees its own release
# as the host's, so it does not recurse).
if [[ -z "${VHS_GUI_IN_CONTAINER:-}" && ( "$container" == 1 || "$release" != "$host_release" ) ]]; then
    image="vhs-gui-deb-build:$release"
    build_cache="$cache/vhs-gui-deb-build/$release"
    mkdir -p "$build_cache/cargo" "$build_cache/target" "$out"
    docker build --build-arg "UBUNTU=$release" -t "$image" -f packaging/Dockerfile.build packaging
    exec docker run --rm --user "$(id -u):$(id -g)" \
        -e HOME=/tmp -e VHS_GUI_IN_CONTAINER=1 -e XDG_CACHE_HOME=/xdg \
        -e CARGO_HOME=/cache/cargo -e CARGO_TARGET_DIR=/cache/target \
        -v "$here:/src" -v "$build_cache:/cache" \
        -v "$plugins:/xdg/vhs-gui-vs-plugins/$release:ro" \
        -w /src "$image" scripts/package-deb.sh "$release"
fi

echo ">>> cargo build --release"
cargo build --release --locked

# Staging goes beside the output, not in /tmp (which is RAM-backed).
mkdir -p "$out"
stage="$(mktemp -d "$out/.stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

install -Dm755 "$target/release/vhs-gui" "$stage/usr/bin/vhs-gui"
install -Dm644 assets/icon.png "$stage/usr/share/icons/hicolor/256x256/apps/vhs-gui.png"
mkdir -p "$stage/usr/share/applications"
sed 's|^Exec=.*|Exec=vhs-gui|' resources/vhs-gui.desktop >"$stage/usr/share/applications/vhs-gui.desktop"
chmod 644 "$stage/usr/share/applications/vhs-gui.desktop"

plugdir="$stage/usr/lib/vhs-gui/vs-plugins"
mkdir -p "$plugdir"
cp -a "$plugins"/libffms2.so* "$plugins/libfmtconv.so" "$plugins/libmiscfilters.so" "$plugdir/"

doc="$stage/usr/share/doc/vhs-gui"
mkdir -p "$doc/licenses"
install -m644 packaging/copyright "$doc/copyright"
install -m644 packaging/licenses/* "$doc/licenses/"
install -m644 "$plugins/REVISIONS" "$doc/vs-plugins-REVISIONS"
install -m644 docs/INSTALL.md "$doc/external-tools.md"
{
    echo "vhs-gui ($version) $(. /etc/os-release && echo "$VERSION_CODENAME"); urgency=medium"
    echo
    echo "  * Built from commit $(git rev-parse --short HEAD)$(git diff --quiet HEAD -- || echo ' (with uncommitted changes)')."
    echo
    echo " -- RyanEiri <RyanEiri@users.noreply.github.com>  $(date -R)"
} | gzip -9n >"$doc/changelog.Debian.gz"
chmod 644 "$doc/changelog.Debian.gz"

# Shared objects are not executable files, and the umask must not leak into the package.
chmod 644 "$plugdir"/*.so*
find "$stage" -type d -exec chmod 755 {} +

# Dependencies: what the binary and the plugins link, worked out by dpkg-shlibdeps, plus the two
# tools vhs-gui runs. dpkg-shlibdeps wants a debian/control to read, so give it an empty one.
shl="$(mktemp -d "$out/.shlibs.XXXXXX")"
trap 'rm -rf "$stage" "$shl"' EXIT
mkdir -p "$shl/debian"
printf 'Source: vhs-gui\n\nPackage: vhs-gui\nArchitecture: amd64\n' >"$shl/debian/control"
shlibs="$(cd "$shl" && dpkg-shlibdeps -O -l"$plugdir" \
    -e"$stage/usr/bin/vhs-gui" \
    -e"$plugdir/libffms2.so.5.0.0" -e"$plugdir/libfmtconv.so" -e"$plugdir/libmiscfilters.so" \
    2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
depends="ffmpeg, sox${shlibs:+, $shlibs}"

size="$(du -sk --exclude=DEBIAN "$stage" | cut -f1)"
mkdir -p "$stage/DEBIAN"
sed -e "s|@VERSION@|$version|" -e "s|@SIZE@|$size|" -e "s|@DEPENDS@|$depends|" \
    packaging/control.in >"$stage/DEBIAN/control"
(cd "$stage" && find . -type f -not -path './DEBIAN/*' -printf '%P\0' | sort -z | xargs -0 md5sum) \
    >"$stage/DEBIAN/md5sums"

rm -f "$out/$pkg.deb"
dpkg-deb --root-owner-group -Zxz --build "$stage" "$out/$pkg.deb"

echo ">>> $out/$pkg.deb"
dpkg-deb --info "$out/$pkg.deb" | sed -n '/Package:/,/Description:/p'
if command -v lintian >/dev/null; then
    echo ">>> lintian"
    lintian --tag-display-limit 0 "$out/$pkg.deb" || true
fi
