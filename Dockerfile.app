# syntax=docker/dockerfile:1
# Aether Gateway runtime image (cross-compilation)
# Binary and frontend assets are pre-built by CI; this Dockerfile only packages them.
# Usage: docker buildx build --platform linux/amd64,linux/arm64 -f Dockerfile.app .
#
# Build context must contain:
#   dist/aether-gateway-amd64   (x86_64-unknown-linux-musl cross-compiled binary)
#   dist/aether-gateway-arm64   (aarch64-unknown-linux-musl cross-compiled binary)
#   dist/frontend/              (npm run build output)

# --- layout stage: create /opt/aether directory structure with symlink ---
# distroless has no shell, so we use busybox to set up the symlink.
FROM busybox:1.37.0-musl@sha256:fc6dddc4c44b1bfe37f41cae8e67d1693828e8f42a91862816d7953e2c9d3f23 AS layout

ARG TARGETARCH

RUN mkdir -p /opt/aether/releases/image/bin /opt/aether/releases/image/frontend /opt/aether/logs

COPY dist/aether-gateway-${TARGETARCH} /opt/aether/releases/image/bin/aether-gateway
COPY dist/frontend/ /opt/aether/releases/image/frontend/

# Keep the immutable release root-owned while guaranteeing that the runtime
# identity can traverse and read every packaged asset.
RUN chmod -R u=rwX,go=rX /opt/aether/releases/image \
    && chmod 0755 /opt/aether/releases/image/bin/aether-gateway

RUN ln -s /opt/aether/releases/image /opt/aether/current

# --- final stage: distroless runtime ---
FROM gcr.io/distroless/static-debian12@sha256:6447365a6337c3732f412d1b74357b30a633831955b2bc45552b0086be907687

COPY --from=layout /opt/aether /opt/aether

WORKDIR /opt/aether

ENV RUST_LOG=aether_gateway=info \
    APP_PORT=8084 \
    HOME=/tmp/aether-home \
    AETHER_UPDATE_STRATEGY=docker \
    AETHER_GATEWAY_STATIC_DIR=/opt/aether/current/frontend

EXPOSE 8084

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD ["/opt/aether/current/bin/aether-gateway", "--healthcheck"]

USER 0:0
ENTRYPOINT ["/opt/aether/current/bin/aether-gateway"]
