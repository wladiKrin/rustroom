#!/bin/bash
# Builds Rustroom's AppImage (v0.1, piece 3c): the app with its own LittleCMS, libde265 and libgphoto2 (its drivers
# too) inside, libheif compiled in, and none of the system's graphics stack. The release workflow runs this same
# script on ubuntu-22.04, and packaging/linux/in-docker.sh runs it in an ubuntu:22.04 container elsewhere.
#
#     packaging/linux/build.sh [--floor <glibc version>] [--target-dir <dir>]
#
# Needs: deps.txt's packages (Ubuntu 22.04), Rust stable, cargo-about (cargo install cargo-about --locked --features
# cli), network once (appimagetool and the AppImage runtime, pinned and checksummed). Writes target/linux/AppDir,
# target/linux/Rustroom-<version>-linux-x86_64.AppImage and its .zsync, and prints them. The glibc the AppImage
# needs is MEASURED — the highest any bundled file asks for, which is that of the machine it was built on — and
# `--floor` refuses an AppImage that needs more (the release passes 2.35, Ubuntu 22.04's). Every step that fails
# stops the script. Design: docs/superpowers/specs/2026-11-18-v0.1-appimage-design.md.
set -euo pipefail

fail() {
    echo "build.sh: $*" >&2
    exit 1
}

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
here="$root/packaging/linux"

floor=""
target_dir="$root/target"
while [ $# -gt 0 ]; do
    case "$1" in
        --floor)
            [ $# -ge 2 ] && [[ "$2" =~ ^[0-9]+\.[0-9]+$ ]] || fail "--floor needs a glibc version such as 2.35, not '${2:-}'"
            floor="$2"
            shift 2
            ;;
        --target-dir)
            [ $# -ge 2 ] && [ -n "$2" ] || fail "--target-dir needs a folder"
            target_dir="$2"
            shift 2
            ;;
        *) fail "unknown argument: $1" ;;
    esac
done
[ "$(uname -m)" = x86_64 ] || fail "the AppImage is built on x86_64 only, not $(uname -m)"

# The version is the app crate's: the AppImage's name, and what a release tag must equal.
pkgid="$(cargo pkgid -p app)" || fail "cargo pkgid failed"
version="${pkgid##*[#@]}"
[ -n "$version" ] || fail "no version for the app crate"
echo "Rustroom $version"

cargo build --release -p app --features appimage --target-dir "$target_dir" || fail "cargo build failed"
bin="$target_dir/release/Rustroom"

out="$root/target/linux"
appdir="$out/AppDir"
lib="$appdir/usr/lib"
camlibs="$lib/libgphoto2/camlibs"
iolibs="$lib/libgphoto2/iolibs"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$camlibs" "$iolibs" "$lib/libheif/plugins"
cp "$bin" "$appdir/usr/bin/Rustroom"

# ⚠ HEVC is in only if libheif's own build found libde265: without it, it builds a libheif that decodes no HEIC,
# silently. The decoder's calls into libde265 are the proof (a plugin name string is inlined away by the compiler).
syms="$(objdump -T "$appdir/usr/bin/Rustroom")" || fail "objdump could not read the binary"
grep -q ' de265_new_decoder$' <<<"$syms" || fail "the binary has no libde265 decoder - was libde265-dev installed?"

# libgphoto2's drivers, found by `rustroom_app::app_bundle` through CAMLIBS and IOLIBS. The four camlibs that are no
# camera — picture frames (ax203, st2205, tp6801) and a pen scanner (docupen) — stay out: they alone link libgd and
# its dozen libraries. So does lumix, which "connects to Lumix cameras over Wifi" — a network port Rustroom's camera
# search never opens — and alone links libcurl with its TLS and Kerberos libraries. The empty usr/lib/libheif/plugins is LIBHEIF_PLUGIN_PATH (plugin loading is off anyway).
# ⚠ Each from the ONE version folder under the library's own folder, never pkg-config's `driverdir`: Arch's
# libgphoto2.pc answers it with the PORT drivers' folder (seen 2026-11-18).
only_dir() {
    local found=()
    local d
    for d in "$1"/*/; do [ -d "$d" ] && found+=("${d%/}"); done
    [ ${#found[@]} -eq 1 ] || fail "expected one version folder in $1, found ${#found[@]}"
    echo "${found[0]}"
}
gphoto_lib="$(pkg-config --variable=libdir libgphoto2)" || fail "pkg-config knows no libgphoto2"
cam_src="$(only_dir "$gphoto_lib/libgphoto2")"
io_src="$(only_dir "$gphoto_lib/libgphoto2_port")"
for f in "$cam_src"/*.so; do
    case "$(basename "$f")" in
        ax203.so | st2205.so | tp6801.so | docupen.so | lumix.so) continue ;;
    esac
    cp "$f" "$camlibs/"
done
cp "$io_src"/*.so "$iolibs/"
[ -f "$camlibs/ptp2.so" ] && [ -f "$iolibs/usb1.so" ] || fail "no ptp2 camlib or usb1 iolib - libgphoto2's drivers are incomplete"

# The libraries every Linux has — AppImage's list, pinned — and never the graphics stack: wgpu and winit load
# Vulkan, GL, Wayland and xkbcommon at run time from the system (dlopen), so they are never NEEDED here; the check
# after the walk makes sure of it.
excluded="$(sed 's/#.*//' "$here/excludelist" | awk 'NF { print $1 }')" || fail "cannot read the excludelist"
[ -n "$excluded" ] || fail "the excludelist is empty"
is_excluded() { grep -qxF "$1" <<<"$excluded"; }

# Every file in the AppDir that is an ELF — the binary, the libraries, the drivers.
elves() {
    local f
    while IFS= read -r -d '' f; do
        [ "$(head -c 4 "$f" | od -An -c | tr -d ' ')" = '177ELF' ] && echo "$f"
    done < <(find "$appdir" -type f -print0)
}

# `ldd`'s answer for one file: "name path" per resolved library, "name not-found" for a missing one. ⚠ The whole
# answer is read into a variable first: a failure inside a pipeline or `< <(…)` would be lost, and an empty answer
# would pass the checks below.
resolved() {
    local out
    out="$(env -i PATH=/usr/bin:/bin ldd "$1" 2>&1)" || return 1
    [ -n "$out" ] || return 1
    # ⚠ The loader may be named by its path (`/usr/lib64/ld-linux-x86-64.so.2 => …`, Arch): its file name is its name.
    awk '$2 == "=>" { n = $1; sub(".*/", "", n); print n, ($3 == "not" ? "not-found" : $3) }' <<<"$out"
}

# The walk, by each file's OWN needs (`readelf`'s NEEDED, resolved by `ldd` on this machine): a library on the list
# is the system's, and so is everything IT needs — libusb brings libudev, which must match the system's too, and
# `ldd`'s flat closure cannot tell whose need a library is. A library the licence map below does not know stops the
# build, so a new dependency is a decision, not an accident.
needed() {
    local out
    out="$(readelf -d "$1")" || return 1
    sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' <<<"$out"
}
queue=()
while IFS= read -r f; do queue+=("$f"); done < <(elves)
[ ${#queue[@]} -gt 2 ] || fail "the AppDir holds almost no ELF files before the walk"
while [ ${#queue[@]} -gt 0 ]; do
    f="${queue[0]}"
    queue=("${queue[@]:1}")
    names="$(needed "$f")" || fail "readelf could not read ${f#"$appdir"/}"
    deps="$(resolved "$f")" || fail "ldd could not read ${f#"$appdir"/}"
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        is_excluded "$name" && continue
        [ -e "$lib/$name" ] && continue
        path="$(awk -v n="$name" '$1 == n { print $2; exit }' <<<"$deps")"
        [ -n "$path" ] && [ "$path" != not-found ] || fail "${f#"$appdir"/} needs $name, which is not on this machine"
        cp -L "$path" "$lib/$name" || fail "could not copy $path"
        queue+=("$lib/$name")
    done <<<"$names"
done

# Each file finds its libraries from where it sits: the binary in ../lib, a library beside itself, a driver two up.
patchelf --set-rpath '$ORIGIN/../lib' "$appdir/usr/bin/Rustroom" || fail "patchelf failed on the binary"
for f in "$lib"/*.so*; do
    [ -f "$f" ] || continue
    chmod u+w "$f"
    patchelf --set-rpath '$ORIGIN' "$f" || fail "patchelf failed on $(basename "$f")"
done
for f in "$camlibs"/*.so "$iolibs"/*.so; do
    chmod u+w "$f"
    patchelf --set-rpath '$ORIGIN/../..' "$f" || fail "patchelf failed on $(basename "$f")"
done

all="$(elves)" || fail "cannot list the AppDir"
[ "$(wc -l <<<"$all")" -gt 2 ] || fail "the AppDir holds almost no ELF files - the walk went wrong"

# Nothing may point outside the AppDir but at the excluded system libraries: resolved with an empty environment,
# every library is found inside it or is on the list.
check_inside() {
    local f deps name path
    while IFS= read -r f; do
        deps="$(resolved "$f")" || fail "ldd could not read ${f#"$appdir"/}"
        while read -r name path; do
            [ -n "$name" ] || continue
            [ "$path" != not-found ] || fail "${f#"$appdir"/}: $name not found"
            case "$path" in "$1"/*) continue ;; esac
            is_excluded "$name" || fail "${f#"$appdir"/} reaches outside the AppImage: $name => $path"
        done <<<"$deps"
    done <<<"$2"
}
check_inside "$appdir" "$all"

# ⚠⚠ Never the graphics stack: a bundled libvulkan or Mesa would load another machine's drivers wrongly or not at all.
for f in "$lib"/*; do
    case "$(basename "$f")" in
        libvulkan* | libGL* | libEGL* | libgbm* | libdrm* | libwayland* | libX* | libxcb* | libxkbcommon* | libglapi*)
            fail "the graphics stack must come from the system, not the AppImage: $(basename "$f")"
            ;;
    esac
done

# The floor, measured: the highest glibc symbol version any bundled file needs, compared as versions.
need=""
needer=""
while IFS= read -r f; do
    t="$(objdump -T "$f")" || fail "objdump could not read ${f#"$appdir"/}"
    v="$(grep -o 'GLIBC_[0-9]*\.[0-9]*\(\.[0-9]*\)\?' <<<"$t" | sed 's/GLIBC_//' | sort -V | tail -1)" || true
    [ -n "$v" ] || continue
    if [ -z "$need" ] || [ "$(printf '%s\n%s\n' "$need" "$v" | sort -V | tail -1)" != "$need" ]; then
        need="$v"
        needer="${f#"$appdir"/}"
    fi
done <<<"$all"
[ -n "$need" ] || fail "no glibc version measured - objdump's answer was empty"
echo "The AppImage needs glibc $need ($needer)"
if [ -n "$floor" ] && [ "$need" != "$floor" ] && [ "$(printf '%s\n%s\n' "$need" "$floor" | sort -V | tail -1)" = "$need" ]; then
    fail "the AppImage needs glibc $need ($needer), more than the required $floor"
fi

# The desktop entry and AppStream metadata, named by the app id — what a desktop matches the window to, and what
# Flathub will read (piece 4). Both validated: a broken one stops the build.
id=io.github.wladikrin.Rustroom
mkdir -p "$appdir/usr/share/applications" "$appdir/usr/share/metainfo"
cp "$here/$id.desktop" "$appdir/$id.desktop"
cp "$here/$id.desktop" "$appdir/usr/share/applications/$id.desktop"
cp "$here/$id.metainfo.xml" "$appdir/usr/share/metainfo/$id.metainfo.xml"
desktop-file-validate "$appdir/$id.desktop" || fail "the desktop file does not validate"
appstreamcli validate --no-net "$appdir/usr/share/metainfo/$id.metainfo.xml" || fail "the AppStream metadata does not validate"
png="$root/packaging/icons/png"
for s in 256 512; do
    mkdir -p "$appdir/usr/share/icons/hicolor/${s}x${s}/apps"
    cp "$png/rustroom-$s.png" "$appdir/usr/share/icons/hicolor/${s}x${s}/apps/$id.png"
done
cp "$png/rustroom-256.png" "$appdir/$id.png"
ln -s "$id.png" "$appdir/.DirIcon"
# The runtime sets APPDIR and APPIMAGE and starts AppRun; `app_bundle` does the rest from inside the app.
ln -s usr/bin/Rustroom "$appdir/AppRun"

# The licences: the GPL, every crate's (cargo-about, as MIT and Apache-2.0 require for a binary), and the native
# libraries' and fonts' own texts — each from its source, never a copy kept in the repository.
licenses="$appdir/usr/share/licenses/rustroom"
mkdir -p "$licenses"
cp "$root/LICENSE" "$licenses/LICENSE"
command -v cargo-about >/dev/null || fail "cargo-about not found (cargo install cargo-about --locked --features cli)"
cargo about generate --manifest-path crates/app/Cargo.toml packaging/about.hbs -o "$licenses/THIRD-PARTY-LICENSES.html" ||
    fail "cargo about failed - is every crate's licence in about.toml?"
crate_dir() {
    local meta
    meta="$(cargo metadata --format-version 1 --filter-platform x86_64-unknown-linux-gnu --features app/appimage)" ||
        return 1
    python3 -c '
import json, os, sys
ps = [p for p in json.load(sys.stdin)["packages"] if p["name"] == sys.argv[1]]
print(os.path.dirname(ps[0]["manifest_path"]) if ps else "")' "$1" <<<"$meta"
}
lensfun_dir="$(crate_dir lensfun)" || fail "cargo metadata failed"
heif_dir="$(crate_dir libheif-sys)" || fail "cargo metadata failed"
[ -n "$lensfun_dir" ] || fail "no lensfun in the dependency graph"
[ -n "$heif_dir" ] || fail "no libheif-sys in the dependency graph"
notice() {
    [ -f "$2" ] || fail "missing licence text: $2"
    cp "$2" "$licenses/$1"
}
notice LibRaw-LICENSE.LGPL.txt "$root/vendor/rsraw-sys/LibRaw/LICENSE.LGPL"
notice LibRaw-LICENSE.CDDL.txt "$root/vendor/rsraw-sys/LibRaw/LICENSE.CDDL"
notice LibRaw-COPYRIGHT.txt "$root/vendor/rsraw-sys/LibRaw/COPYRIGHT"
notice LensFun-NOTICE.txt "$lensfun_dir/NOTICE"
notice Inter-LICENSE.txt "$root/crates/app/assets/Inter-LICENSE"
notice Phosphor-LICENSE.txt "$root/crates/app/assets/Phosphor-LICENSE"
# libheif is linked in statically from libheif-sys's own copy of its source; its LGPL text is still owed.
notice libheif-COPYING.txt "$heif_dir/vendor/libheif/COPYING"

# Each bundled library's own text, from the Ubuntu package that ships it (Debian's copyright file names every
# licence in the package). A library this map does not know stops the build: a new one needs its text found by a
# person — and it is how a codec that libheif's build picked up by accident is caught.
for f in "$lib"/*.so*; do
    name="$(basename "$f")"
    case "$name" in
        libde265.so.*) pkg=libde265-0 ;;
        libexif.so.*) pkg=libexif12 ;;
        libgphoto2.so.* | libgphoto2_port.so.*) pkg=libgphoto2-6 ;;
        liblcms2.so.*) pkg=liblcms2-2 ;;
        libltdl.so.*) pkg=libltdl7 ;;
        *) fail "no licence text for $name" ;;
    esac
    notice "${name%%.so*}-copyright.txt" "/usr/share/doc/$pkg/copyright"
done

# appimagetool and the AppImage runtime it embeds, both pinned: appimagetool would otherwise fetch the runtime's
# moving "continuous" build at every run.
tools="$out/tools"
mkdir -p "$tools"
fetch() {
    local url="$1" file="$2" sum="$3"
    if [ ! -f "$tools/$file" ] || ! echo "$sum  $tools/$file" | sha256sum -c --quiet - 2>/dev/null; then
        curl -fsSL --retry 3 -o "$tools/$file" "$url" || fail "could not download $file"
    fi
    echo "$sum  $tools/$file" | sha256sum -c --quiet - || fail "$file does not match its checksum"
}
fetch https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
    appimagetool-x86_64.AppImage ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
fetch https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
    runtime-x86_64 2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
chmod +x "$tools/appimagetool-x86_64.AppImage"
command -v zsyncmake >/dev/null || fail "zsyncmake not found (the zsync package) - the update file would be missing"

# The AppImage, with where its updates come from: this project's GitHub releases (Gear Lever, AppImageUpdate).
image="$out/Rustroom-$version-linux-x86_64.AppImage"
rm -f "$image" "$image.zsync"
# ⚠ --appimage-extract-and-run: no FUSE inside a container.
(cd "$out" && ARCH=x86_64 "$tools/appimagetool-x86_64.AppImage" --appimage-extract-and-run \
    --runtime-file "$tools/runtime-x86_64" \
    -u 'gh-releases-zsync|wladiKrin|rustroom|latest|Rustroom-*-linux-x86_64.AppImage.zsync' \
    "$appdir" "$image") || fail "appimagetool failed"
[ -f "$image" ] && [ -f "$image.zsync" ] || fail "appimagetool wrote no AppImage or no .zsync"

# The same check on what was packed, not only on what was meant to be: extract it and resolve again.
check="$out/check"
rm -rf "$check"
mkdir -p "$check"
(cd "$check" && "$image" --appimage-extract >/dev/null) || fail "the AppImage does not extract"
appdir_was="$appdir"
appdir="$check/squashfs-root"
packed="$(elves)" || fail "cannot list the extracted AppImage"
[ "$(wc -l <<<"$packed")" -eq "$(wc -l <<<"$all")" ] || fail "the AppImage holds a different set of ELF files than its AppDir"
check_inside "$appdir" "$packed"
appdir="$appdir_was"
rm -rf "$check"

echo "$image"
echo "$image.zsync"
