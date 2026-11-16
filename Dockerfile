# Builds the audioserver as one static musl binary in a minimal image.
#   container build --platform linux/arm64 -t loxone-audioserver:latest .
# or push it to a registry for a MikroTik App (see README).
# On an Apple Silicon Mac this is a native build (no emulation).
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev build-base cmake perl
WORKDIR /src

# 1. Dependencies only. This layer depends on Cargo.toml and Cargo.lock alone, so it is
#    reused (locally, and from the CI build cache) until a dependency changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked \
 && cargo clean --release -p loxone-audioserver \
 && rm -rf src

# 2. The crate. A code change only rebuilds from here. The sources are touched because COPY
#    keeps their original timestamps, which can be older than the stub build above.
COPY src src
RUN find src -type f -exec touch {} + \
 && cargo build --release --locked \
 && mkdir /out && cp target/release/loxone-audioserver /out/ \
 && /out/loxone-audioserver --help | grep -q 'Software Loxone Audioserver' \
 || { echo 'the built binary is not the real loxone-audioserver' >&2; exit 1; }

FROM scratch
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /out/loxone-audioserver /loxone-audioserver
# Config, pairing key, recents, custom radios and voice messages are written relative to
# the working directory, so they all land in the /data volume.
WORKDIR /data
VOLUME /data
EXPOSE 7091/tcp 7095/tcp 7070/udp
# Same discovery name as the earlier image, which passed --name "Loxone Audioserver".
ENV RUST_LOG=info AUDIOSERVER_NAME="Loxone Audioserver"
# Settings come from AUDIOSERVER_SERIAL (required), AUDIOSERVER_NAME, AUDIOSERVER_HOSTNAME and
# AUDIOSERVER_PUBLIC_HOST, or from the matching --flags after the image name: the binary is the
# entrypoint, as in the earlier image, so `run IMAGE --serial X --public-host Y` keeps working.
# Keep the healthcheck in sync with mikrotik/loxone-audioserver.yaml.
HEALTHCHECK --interval=60s --timeout=10s --start-period=30s CMD ["/loxone-audioserver", "health"]
ENTRYPOINT ["/loxone-audioserver"]
