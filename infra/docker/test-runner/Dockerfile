ARG RUST_IMAGE
FROM ${RUST_IMAGE}

ARG DEBIAN_SNAPSHOT
ARG NODE_VERSION
ARG NODE_URL
ARG NODE_SHA256
ARG TAURI_CLI_VERSION
ARG TAURI_CLI_URL
ARG TAURI_CLI_SHA512
ARG RUST_TOOLCHAIN_DIR
ARG AFK_UID
ARG AFK_GID
RUN printf '%s\n' \
      "deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT} bookworm main" \
      "deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT} bookworm-security main" \
      > /etc/apt/sources.list \
    && rm -f /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl dbus-x11 file fonts-dejavu-core \
      libayatana-appindicator3-dev librsvg2-dev libssl-dev libwebkit2gtk-4.1-dev \
      libxdo-dev webkit2gtk-driver wget xauth xvfb xz-utils \
    && rm -rf /var/lib/apt/lists/*
ADD ${NODE_URL} /tmp/node.tar.xz
ADD ${TAURI_CLI_URL} /tmp/tauri-cli.tgz
RUN echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - \
    && echo "${TAURI_CLI_SHA512}  /tmp/tauri-cli.tgz" | sha512sum -c - \
    && tar -xJf /tmp/node.tar.xz --strip-components=1 -C /usr/local \
    && rm /tmp/node.tar.xz \
    && npm install --global "/tmp/tauri-cli.tgz" \
    && rm /tmp/tauri-cli.tgz \
    && npm cache clean --force

# Bypass rustup's writable proxy state at runtime. The lab mounts a disposable
# HOME and runs with a read-only root filesystem, so the pinned toolchain's real
# binaries must be first on PATH.
ENV PATH="/usr/local/rustup/toolchains/${RUST_TOOLCHAIN_DIR}/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

LABEL org.opencontainers.image.description="Pinned AFK Rust/Tauri/WebKitGTK/X11 test runner"
WORKDIR /afk/runner
USER ${AFK_UID}:${AFK_GID}
