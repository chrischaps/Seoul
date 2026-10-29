# A Debian bookworm build box for Seoul's Linux binaries, native x86_64 and
# cross-compiled aarch64 (Raspberry Pi 5). Bookworm's glibc is the oldest
# Seoul targets, so a binary built here runs on Pi OS bookworm and newer.
#   docker build -t seoul-linux -f packaging/linux.Dockerfile packaging
#   docker run --rm -v "$PWD:/src" -w /src seoul-linux ./packaging/package-linux.sh aarch64
FROM rust:1-bookworm
RUN dpkg --add-architecture arm64 \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config libasound2-dev libasound2-dev:arm64 gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add aarch64-unknown-linux-gnu
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    PKG_CONFIG_PATH_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig \
    PKG_CONFIG_ALLOW_CROSS=1
