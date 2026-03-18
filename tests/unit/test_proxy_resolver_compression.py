from __future__ import annotations

import gzip
import json

import pytest

from src.services.proxy_node.resolver import (
    build_post_kwargs,
    build_post_kwargs_async,
    build_stream_kwargs,
    build_stream_kwargs_async,
    resolve_ops_proxy_config_async,
    resolve_proxy_info_async,
)


class TestProxyResolverCompression:
    def test_build_post_kwargs_compresses_when_client_sent_gzip(self) -> None:
        payload = {"message": "hello", "tokens": [1, 2, 3]}
        kwargs = build_post_kwargs(
            None,
            url="https://example.com/v1/messages",
            headers={"Content-Type": "application/json"},
            payload=payload,
            timeout=10.0,
            client_content_encoding="gzip",
        )

        assert kwargs["headers"]["Content-Encoding"] == "gzip"
        assert json.loads(gzip.decompress(kwargs["content"]).decode("utf-8")) == payload

    def test_build_post_kwargs_keeps_plain_body_without_client_gzip(self) -> None:
        payload = {"message": "plain"}
        kwargs = build_post_kwargs(
            None,
            url="https://example.com/v1/messages",
            headers={"Content-Type": "application/json"},
            payload=payload,
            timeout=10.0,
            client_content_encoding=None,
        )

        assert "Content-Encoding" not in kwargs["headers"]
        assert json.loads(kwargs["content"].decode("utf-8")) == payload

    def test_build_stream_kwargs_drops_stale_content_encoding_header(self) -> None:
        kwargs = build_stream_kwargs(
            None,
            url="https://example.com/v1/messages",
            headers={"content-encoding": "gzip", "Content-Type": "application/json"},
            payload={"message": "no-gzip"},
            timeout=10.0,
            client_content_encoding=None,
        )

        assert all(key.lower() != "content-encoding" for key in kwargs["headers"])

    @pytest.mark.asyncio
    async def test_build_post_kwargs_async_compresses_when_client_sent_gzip(self) -> None:
        payload = {"message": "hello", "tokens": [1, 2, 3]}

        kwargs = await build_post_kwargs_async(
            None,
            url="https://example.com/v1/messages",
            headers={"Content-Type": "application/json"},
            payload=payload,
            timeout=10.0,
            client_content_encoding="gzip",
        )

        assert kwargs["headers"]["Content-Encoding"] == "gzip"
        assert json.loads(gzip.decompress(kwargs["content"]).decode("utf-8")) == payload

    @pytest.mark.asyncio
    async def test_build_stream_kwargs_async_drops_stale_content_encoding_header(self) -> None:
        kwargs = await build_stream_kwargs_async(
            None,
            url="https://example.com/v1/messages",
            headers={"content-encoding": "gzip", "Content-Type": "application/json"},
            payload={"message": "no-gzip"},
            timeout=10.0,
            client_content_encoding=None,
        )

        assert all(key.lower() != "content-encoding" for key in kwargs["headers"])

    @pytest.mark.asyncio
    async def test_resolve_proxy_info_async_masks_manual_proxy_url(self) -> None:
        info = await resolve_proxy_info_async(
            {
                "enabled": True,
                "url": "socks5://user:pass@proxy.example.com:1080",
            }
        )

        assert info == {
            "url": "socks5://proxy.example.com:1080",
            "source": "provider",
        }

    @pytest.mark.asyncio
    async def test_resolve_ops_proxy_config_async_preserves_legacy_proxy(
        self,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setattr(
            "src.services.proxy_node.resolver.get_system_proxy_config",
            lambda: None,
        )
        proxy, tunnel_node_id = await resolve_ops_proxy_config_async(
            {"proxy": "http://proxy.example.com:8080"}
        )

        assert proxy == "http://proxy.example.com:8080"
        assert tunnel_node_id is None
