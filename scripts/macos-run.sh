#!/bin/sh
# The cargo runner on macOS (`.cargo/config.toml`): signs the `Rustroom` binary with the local
# "Rustroom Dev" code-signing identity before running it, so the keychain's "Always Allow" for the
# Immich keys survives rebuilds (M10.15a) — an ad-hoc signature changes with every build, and macOS
# asks for the password again each time. Every other binary (tests, examples) runs as it is, and
# without the identity (another Mac, CI) Rustroom runs unsigned exactly as before.
bin="$1"
if [ "$(basename "$bin")" = "Rustroom" ] &&
    security find-identity -p codesigning 2>/dev/null | grep -q '"Rustroom Dev"'; then
    codesign --force -s "Rustroom Dev" -i com.rustroom.dev "$bin" 2>/dev/null ||
        echo "rustroom: could not sign with \"Rustroom Dev\"; running unsigned" >&2
fi
exec "$@"
