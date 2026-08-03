# Build + test image for guac-rs.
#
# The crate only targets Linux and builds guacamole-server (libguac + the
# VNC/RDP/SSH plugins) from source, so it needs the full C toolchain and the
# protocol development libraries. This host (macOS) can't build it natively;
# everything runs here.
#
#   docker build -t guac-rs .
#   docker run --rm guac-rs                       # cargo test
#   docker run --rm guac-rs cargo build           # or any other command
FROM rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config autoconf automake libtool libtool-bin \
        clang libclang-dev \
        ca-certificates curl \
        libcairo2-dev libjpeg62-turbo-dev libpng-dev libwebp-dev \
        libpango1.0-dev uuid-dev libossp-uuid-dev libssl-dev \
        libvncserver-dev \
        freerdp2-dev libwinpr2-dev \
        libssh2-1-dev \
        tigervnc-standalone-server \
    && rm -rf /var/lib/apt/lists/*

# clippy/rustfmt are not in the base image by default.
RUN rustup component add clippy rustfmt

WORKDIR /src
COPY . /src

CMD ["cargo", "test", "--", "--nocapture"]
