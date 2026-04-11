# syntax=docker/dockerfile:1
# Aether Gateway 运行时镜像（交叉编译方案）
# 二进制和前端产物均由 CI 预先构建，此 Dockerfile 仅做打包
# 用法: docker buildx build --platform linux/amd64,linux/arm64 -f Dockerfile.app .
#
# 构建上下文中须包含:
#   dist/aether-gateway-amd64   (x86_64-unknown-linux-musl 交叉编译产物)
#   dist/aether-gateway-arm64   (aarch64-unknown-linux-musl 交叉编译产物)
#   dist/frontend/              (npm run build 产物)

FROM gcr.io/distroless/static-debian12

# TARGETARCH 由 buildx 自动注入: amd64 或 arm64
ARG TARGETARCH

COPY dist/aether-gateway-${TARGETARCH} /usr/local/bin/aether-gateway
COPY dist/frontend/ /srv/frontend

WORKDIR /app

ENV RUST_LOG=aether_gateway=info \
    AETHER_GATEWAY_BIND=0.0.0.0:8084 \
    AETHER_GATEWAY_STATIC_DIR=/srv/frontend

EXPOSE 8084

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/aether-gateway", "--healthcheck"]

USER root
ENTRYPOINT ["/usr/local/bin/aether-gateway"]
