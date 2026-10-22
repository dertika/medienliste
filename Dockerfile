# syntax=docker/dockerfile:1
#
# Multi-Arch-Build (linux/amd64, linux/arm64) ohne Emulation:
# Cross-Kompilierung mit cargo-zigbuild zu einem statischen musl-Binary.

ARG RUST_VERSION=1.97

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-alpine AS build
ARG TARGETARCH
RUN apk add --no-cache musl-dev zig \
 && cargo install --locked cargo-zigbuild
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "Nicht unterstützte Architektur: $TARGETARCH" >&2; exit 1 ;; \
    esac > /rust-target \
 && rustup target add "$(cat /rust-target)"
WORKDIR /src
COPY . .
# Eigene Caches pro Architektur: die parallelen Builds dürfen sich die Registry nicht teilen,
# sonst kollidieren sie beim Entpacken der Crates (Cargos Lock liegt außerhalb des Caches).
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-$TARGETARCH \
    --mount=type=cache,target=/src/target,id=target-$TARGETARCH \
    cargo zigbuild --release --locked --target "$(cat /rust-target)" \
 && cp "target/$(cat /rust-target)/release/medienliste" /medienliste \
 && mkdir -p /data-skel

FROM gcr.io/distroless/static-debian12:nonroot
LABEL org.opencontainers.image.title="Medienliste" \
      org.opencontainers.image.description="Selbst gehostete Webapp zum Tracken von Büchern, Hörbüchern und Filmen"
COPY --from=build /medienliste /usr/local/bin/medienliste
COPY --from=build --chown=65532:65532 /data-skel /data
ENV MEDIENLISTE_DATA_DIR=/data \
    MEDIENLISTE_LISTEN=0.0.0.0:8080
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/medienliste", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/medienliste"]
CMD ["serve"]
