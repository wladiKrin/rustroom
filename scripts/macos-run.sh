#!/bin/sh
# The cargo runner on macOS (`.cargo/config.toml`): signs the `Rustroom` binary with the local
# "Rustroom Dev" code-signing identity before running it (M10.15a), so the keychain item's trusted
# application — a requirement on that identity — stays the same across rebuilds.
# ⚠⚠ It does NOT stop the prompt (measured 2026-10-13): a self-signed certificate has no Team ID, so
# macOS also ties the item to each build's cdhash (its partition list), and the first launch after a
# rebuild asks for the login password once. Accepted by the author; a certificate with a Team ID
# (Apple Development, Developer ID) would end it. Every other binary (tests, examples) runs as it is,
# and without the identity (another Mac, CI) Rustroom runs unsigned exactly as before.
bin="$1"
if [ "$(basename "$bin")" = "Rustroom" ] &&
    security find-identity -p codesigning 2>/dev/null | grep -q '"Rustroom Dev"'; then
    codesign --force -s "Rustroom Dev" -i com.rustroom.dev "$bin" 2>/dev/null ||
        echo "rustroom: could not sign with \"Rustroom Dev\"; running unsigned" >&2
fi
exec "$@"
