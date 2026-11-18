#!/bin/bash
# Builds Rustroom's Mac app (v0.1, piece 3b): Rustroom.app with its Homebrew libraries and libgphoto2's drivers
# inside, ad hoc signed. The release workflow runs this same script, so a local build and a release cannot differ.
#
#     packaging/macos/build.sh [--floor <macOS version>]
#
# Needs: Homebrew's little-cms2, libheif, libgphoto2 and pkgconf; macOS's own otool, install_name_tool, codesign,
# iconutil, plutil. Writes target/macos/Rustroom.app. The app's minimum macOS is MEASURED — the highest any file in the
# bundle needs, which is that of the machine's Homebrew — and `--floor` refuses a bundle that needs more (the release
# passes 15.0). Every step that fails stops the script. Design: docs/superpowers/specs/2026-11-18-v0.1-packaging-mac-design.md.
set -euo pipefail

fail() {
    echo "build.sh: $*" >&2
    exit 1
}

# ⚠ Apple's own tools, never the first on PATH: Anaconda and MacPorts install cctools ports of otool and
# install_name_tool ahead of /usr/bin, and those write fake signatures of their own (seen on the author's Mac).
otool() { /usr/bin/otool "$@"; }
install_name_tool() { /usr/bin/install_name_tool "$@"; }

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

floor=""
while [ $# -gt 0 ]; do
    case "$1" in
        --floor)
            [ $# -ge 2 ] || fail "--floor needs a macOS version, such as 15.0"
            floor="$2"
            shift 2
            ;;
        *) fail "unknown argument: $1" ;;
    esac
done

command -v brew >/dev/null || fail "Homebrew (brew) not found"
brew_prefix="$(brew --prefix)"
gphoto_prefix="$(brew --prefix libgphoto2)"
export PKG_CONFIG_PATH="$brew_prefix/lib/pkgconfig:$(brew --prefix little-cms2)/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

# The version is the app crate's: the bundle's, the disk image's name, and what a release tag must equal.
pkgid="$(cargo pkgid -p app)" || fail "cargo pkgid failed"
version="${pkgid##*[#@]}"
[ -n "$version" ] || fail "no version for the app crate"
echo "Rustroom $version"

cargo build --release -p app || fail "cargo build failed"

out="$root/target/macos"
app="$out/Rustroom.app"
contents="$app/Contents"
frameworks="$contents/Frameworks"
camlibs="$contents/PlugIns/libgphoto2/camlibs"
iolibs="$contents/PlugIns/libgphoto2/iolibs"
rm -rf "$out"
mkdir -p "$contents/MacOS" "$contents/Resources" "$frameworks" "$camlibs" "$iolibs" "$contents/PlugIns/libheif"
cp "$root/target/release/Rustroom" "$contents/MacOS/Rustroom"

# libgphoto2's drivers, found by `rustroom_app::app_bundle` through CAMLIBS and IOLIBS. The four camlibs that are no
# camera — picture frames (ax203, st2205, tp6801) and a pen scanner (docupen) — stay out: they alone link libgd and
# its dozen libraries. The empty PlugIns/libheif is LIBHEIF_PLUGIN_PATH: no plugin from outside the bundle.
only_dir() {
    local found=()
    local d
    for d in "$1"/*/; do [ -d "$d" ] && found+=("${d%/}"); done
    [ ${#found[@]} -eq 1 ] || fail "expected one version folder in $1, found ${#found[@]}"
    echo "${found[0]}"
}
cam_src="$(only_dir "$gphoto_prefix/lib/libgphoto2")"
io_src="$(only_dir "$gphoto_prefix/lib/libgphoto2_port")"
for f in "$cam_src"/*.so; do
    case "$(basename "$f")" in
        ax203.so | st2205.so | tp6801.so | docupen.so) continue ;;
    esac
    cp "$f" "$camlibs/"
done
cp "$io_src"/*.so "$iolibs/"
chmod u+w "$camlibs"/*.so "$iolibs"/*.so

# A Mach-O's own install name (a library's), its references, its LC_RPATH entries.
own_id() { otool -D "$1" | tail -n +2; }
references() {
    local id
    id="$(own_id "$1")"
    otool -L "$1" | tail -n +2 | sed -E 's/^[[:space:]]+//; s/ \(compatibility version.*$//' |
        while IFS= read -r ref; do [ "$ref" = "$id" ] || echo "$ref"; done
}
rpaths() { otool -l "$1" | awk '/cmd LC_RPATH/ { r = 1 } r && $1 == "path" { print $2; r = 0 }'; }
is_system() {
    case "$1" in /System/* | /usr/lib/*) return 0 ;; *) return 1 ;; esac
}

# Where a reference made by SOURCE (the file's ORIGINAL place, which its @loader_path means) points.
resolve() {
    local ref="$1" source="$2"
    local dir rp
    dir="$(dirname "$source")"
    case "$ref" in
        @rpath/*)
            while IFS= read -r rp; do
                rp="${rp/@loader_path/$dir}"
                if [ -e "$rp/${ref#@rpath/}" ]; then
                    echo "$rp/${ref#@rpath/}"
                    return
                fi
            done < <(rpaths "$source")
            fail "$source: cannot resolve $ref through its rpaths"
            ;;
        @loader_path/*) echo "$dir/${ref#@loader_path/}" ;;
        /*) echo "$ref" ;;
        *) fail "$source: a reference this script cannot follow: $ref" ;;
    esac
}

# The walk: from the binary and every driver, each non-system library — copied once, by the name it is asked for,
# then walked in turn from its original place.
sources=("$root/target/release/Rustroom")
for f in "$camlibs"/*.so; do sources+=("$cam_src/$(basename "$f")"); done
for f in "$iolibs"/*.so; do sources+=("$io_src/$(basename "$f")"); done
i=0
while [ $i -lt ${#sources[@]} ]; do
    source="${sources[$i]}"
    i=$((i + 1))
    while IFS= read -r ref; do
        is_system "$ref" && continue
        name="$(basename "$ref")"
        [ -e "$frameworks/$name" ] && continue
        target="$(resolve "$ref" "$source")"
        [ -e "$target" ] || fail "$source needs $ref, which does not exist"
        cp -L "$target" "$frameworks/$name"
        chmod u+w "$frameworks/$name"
        sources+=("$target")
    done < <(references "$source")
done

machos() {
    echo "$contents/MacOS/Rustroom"
    find "$frameworks" "$contents/PlugIns" -type f \( -name '*.dylib' -o -name '*.so' \) | sort
}

# install_name_tool, without its notice that the signature it changes will be invalid (everything is signed below).
relink() {
    local err
    if ! err="$(install_name_tool "$@" 2>&1)"; then
        fail "install_name_tool $*: $err"
    fi
    echo "$err" | grep -v -e 'invalidate the code signature' -e '^$' >&2 || true
}

# The rewrite: every reference to @rpath/<name>, every old rpath gone, one new rpath per kind of file.
while IFS= read -r m; do
    args=()
    case "$m" in *.dylib) args+=(-id "@rpath/$(basename "$m")") ;; esac
    while IFS= read -r ref; do
        is_system "$ref" || args+=(-change "$ref" "@rpath/$(basename "$ref")")
    done < <(references "$m")
    while IFS= read -r rp; do args+=(-delete_rpath "$rp"); done < <(rpaths "$m")
    [ ${#args[@]} -eq 0 ] || relink "${args[@]}" "$m"
    case "$m" in
        "$contents/MacOS/"*) relink -add_rpath "@executable_path/../Frameworks" "$m" ;;
        "$frameworks/"*) relink -add_rpath "@loader_path" "$m" ;;
        *) relink -add_rpath "@loader_path/../../../Frameworks" "$m" ;;
    esac
done < <(machos)

# Nothing may point outside the bundle and the system.
while IFS= read -r m; do
    while IFS= read -r p; do
        case "$p" in
            /System/* | /usr/lib/* | @rpath/* | @executable_path/* | @loader_path | @loader_path/*) ;;
            *) fail "${m#"$app"/} still names $p" ;;
        esac
    done < <(own_id "$m"; references "$m"; rpaths "$m")
done < <(machos)

# The floor, measured: the highest minimum macOS any file in the bundle was built for.
min_macos() {
    otool -l "$1" | awk '
        /cmd LC_BUILD_VERSION/ { b = 1 } b && $1 == "minos" { print $2; exit }
        /cmd LC_VERSION_MIN_MACOSX/ { v = 1 } v && $1 == "version" { print $2; exit }'
}
need=""
needers=""
while IFS= read -r m; do
    v="$(min_macos "$m")"
    [ -n "$v" ] || fail "${m#"$app"/} names no minimum macOS"
    if [ -z "$need" ] || [ "$(printf '%s\n%s\n' "$need" "$v" | sort -V | tail -1)" != "$need" ]; then
        need="$v"
        needers="$(basename "$m")"
    elif [ "$v" = "$need" ]; then
        needers="$needers $(basename "$m")"
    fi
done < <(machos)
echo "The bundle needs macOS $need ($(echo "$needers" | wc -w | tr -d ' ') files, among them $(echo "$needers" | cut -d' ' -f1-3))"
if [ -n "$floor" ] && [ "$(printf '%s\n%s\n' "$floor" "$need" | sort -V | tail -1)" != "$floor" ]; then
    fail "the bundle needs macOS $need ($(echo "$needers" | cut -d' ' -f1)), more than the required $floor"
fi

sed -e "s/@VERSION@/$version/g" -e "s/@MIN_MACOS@/$need/g" "$root/packaging/macos/Info.plist" >"$contents/Info.plist"
plutil -lint "$contents/Info.plist" >/dev/null || fail "Info.plist is not a valid property list"

# The icon, from the committed PNGs (packaging/icons/): an .iconset, then iconutil.
iconset="$out/rustroom.iconset"
mkdir -p "$iconset"
png="$root/packaging/icons/png"
for s in 16 32 128 256 512; do
    cp "$png/rustroom-$s.png" "$iconset/icon_${s}x${s}.png"
    cp "$png/rustroom-$((s * 2)).png" "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o "$contents/Resources/rustroom.icns" "$iconset" || fail "iconutil failed"
rm -rf "$iconset"

# Signing, ad hoc: Apple Silicon runs no unsigned code. Every library and driver first, then the app.
while IFS= read -r m; do
    [ "$m" = "$contents/MacOS/Rustroom" ] && continue
    codesign --force --sign - --timestamp=none "$m" 2>/dev/null || fail "codesign failed on ${m#"$app"/}"
done < <(machos)
codesign --force --sign - --timestamp=none "$app" || fail "codesign failed on the app"
codesign --verify --deep --strict --verbose=2 "$app" || fail "the app's signature does not verify"

echo "$app"
