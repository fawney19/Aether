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
FROM busybox:1.37-musl AS layout

ARG TARGETARCH

RUN mkdir -p /opt/aether/releases/image/bin /opt/aether/releases/image/frontend /opt/aether/logs

COPY dist/aether-gateway-${TARGETARCH} /opt/aether/releases/image/bin/aether-gateway
RUN chmod 0755 /opt/aether/releases/image/bin/aether-gateway
COPY dist/frontend/ /opt/aether/releases/image/frontend/

RUN ln -s /opt/aether/releases/image /opt/aether/current

# --- final stage: distroless runtime ---
FROM gcr.io/distroless/static-debian12

COPY --from=layout /opt/aether /opt/aether

WORKDIR /opt/aether

ENV RUST_LOG=aether_gateway=info \
    APP_PORT=8084 \
    AETHER_UPDATE_STRATEGY=docker \
    AETHER_GATEWAY_STATIC_DIR=/opt/aether/current/frontend

EXPOSE 8084

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD ["/opt/aether/current/bin/aether-gateway", "--healthcheck"]

USER root
ENTRYPOINT ["/opt/aether/current/bin/aether-gateway"]
