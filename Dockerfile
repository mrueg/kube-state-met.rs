# kube-state-met.rs: static musl binary on distroless (ADR-009).
# docker buildx build --platform linux/amd64,linux/arm64 -t kube-state-metrics-rs .
FROM --platform=$BUILDPLATFORM rust:1.100-bookworm AS build
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl > /target ;; \
      arm64) echo aarch64-unknown-linux-musl > /target ;; \
      *) echo "unsupported arch $TARGETARCH" && exit 1 ;; \
    esac \
 && rustup target add "$(cat /target)" \
 && apt-get update && apt-get install -y --no-install-recommends musl-tools gcc-aarch64-linux-gnu \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc
RUN cargo build --release --locked -p kube-state-metrics --target "$(cat /target)" \
 && cp "target/$(cat /target)/release/kube-state-metrics" /kube-state-metrics

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /kube-state-metrics /kube-state-metrics
USER 65534
ENTRYPOINT ["/kube-state-metrics", "--port=8080", "--telemetry-port=8081"]
EXPOSE 8080 8081
