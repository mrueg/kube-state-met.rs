# kube-state-met.rs: static musl binary on distroless (ADR-009).
#   docker buildx build --platform linux/amd64,linux/arm64 -t kube-state-metrics-rs .
# The build stage runs on the build host and cross-compiles with cargo-zigbuild
# (zig is the C compiler for ring and the linker), so no emulation is needed.
FROM --platform=$BUILDPLATFORM rust:1.100-bookworm AS build
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends python3-pip \
 && rm -rf /var/lib/apt/lists/* \
 && pip3 install --break-system-packages --no-cache-dir ziglang==0.15.1 \
 && cargo install --locked cargo-zigbuild
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl > /target ;; \
      arm64) echo aarch64-unknown-linux-musl > /target ;; \
      *) echo "unsupported arch $TARGETARCH" && exit 1 ;; \
    esac \
 && rustup target add "$(cat /target)"
WORKDIR /src
COPY . .
RUN cargo zigbuild --release --locked -p kube-state-metrics --target "$(cat /target)" \
 && cp "target/$(cat /target)/release/kube-state-metrics" /kube-state-metrics

FROM gcr.io/distroless/static-debian12:nonroot
LABEL org.opencontainers.image.source="https://github.com/mrueg/kube-state-met.rs"
COPY --from=build /kube-state-metrics /kube-state-metrics
USER 65534
ENTRYPOINT ["/kube-state-metrics", "--port=8080", "--telemetry-port=8081"]
EXPOSE 8080 8081
