# The AppImage's build image (v0.1, piece 3c): Ubuntu 22.04 — glibc 2.35, the AppImage's floor — with deps.txt, Rust
# stable and cargo-about. Used by packaging/linux/in-docker.sh; the release workflow installs the same list on its
# ubuntu-22.04 runner instead.
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
COPY deps.txt /tmp/deps.txt
RUN apt-get update \
    && grep -v '^#' /tmp/deps.txt | xargs apt-get install -y --no-install-recommends \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | CARGO_HOME=/usr/local/cargo sh -s -- -y --profile minimal --default-toolchain stable \
    && CARGO_HOME=/usr/local/cargo cargo install cargo-about --locked --features cli \
    && chmod -R a+rX /usr/local/cargo /usr/local/rustup
