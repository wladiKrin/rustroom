#!/bin/bash
# Builds the AppImage on any Linux with Docker (v0.1, piece 3c): packaging/linux/build.sh inside Ubuntu 22.04, so
# its glibc floor is 22.04's whatever the host runs. Arguments go to build.sh.
#
#     packaging/linux/in-docker.sh [--floor <glibc version>]
#
# The checkout is mounted at /src and the files written are the caller's. Cargo's registry and the build live under
# target/linux-docker/, apart from the host's own target/release (a different glibc).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
image=rustroom-appimage-build
docker build -t "$image" "$root/packaging/linux" || { echo "in-docker.sh: docker build failed" >&2; exit 1; }
mkdir -p "$root/target/linux-docker/cargo-home"
exec docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v "$root:/src" -w /src \
    -e CARGO_HOME=/src/target/linux-docker/cargo-home \
    -e HOME=/src/target/linux-docker \
    "$image" packaging/linux/build.sh --target-dir /src/target/linux-docker/target "$@"
