# Linux release packaging image (ADR-0030). Ubuntu 22.04 sets the glibc floor of the packages (2.35): they run
# on Ubuntu 22.04+, Debian 12+, Fedora 36+ and similar. Used by `cargo xtask package --docker`, which runs it
# as the calling user with the repository mounted at /work.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config curl ca-certificates git file xz-utils \
        libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libfontconfig-dev libfreetype-dev \
        libx11-xcb-dev libxcb1-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*

# Toolchain and tools live outside the (mounted) home so any user id can run them. Keep RUST_VERSION in step
# with rust-toolchain.toml, PACKAGER_VERSION and CARGO_ABOUT_VERSION with .github/workflows/release.yml.
# Ubuntu 22.04's libheif (1.12) is older than the `heic` feature needs, so these packages build without it and
# say so in the release notes (ADR-0059).
ARG RUST_VERSION=1.99.0
ARG PACKAGER_VERSION=0.11.8
ARG CARGO_ABOUT_VERSION=0.9.2
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "$RUST_VERSION" --component rustfmt,clippy \
    && cargo install cargo-packager --version "$PACKAGER_VERSION" --locked \
    && cargo install cargo-about --version "$CARGO_ABOUT_VERSION" --features cli --locked \
    && rm -rf /opt/cargo/registry /opt/cargo/git \
    && chmod -R a+rX /opt/rustup /opt/cargo

# linuxdeploy and appimagetool are AppImages themselves; containers have no FUSE to mount them.
ENV APPIMAGE_EXTRACT_AND_RUN=1
