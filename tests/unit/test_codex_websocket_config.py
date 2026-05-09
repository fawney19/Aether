from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def test_frontend_codex_config_advertises_websocket_support() -> None:
    content = (ROOT / "frontend/src/views/public/home-config.ts").read_text(encoding="utf-8")

    assert 'wire_api = "responses"' in content
    assert "supports_websockets = true" in content


def test_nginx_api_proxy_forwards_websocket_upgrade() -> None:
    for dockerfile in ["Dockerfile.app", "Dockerfile.app.local"]:
        content = (ROOT / dockerfile).read_text(encoding="utf-8")

        assert "map $http_upgrade $connection_upgrade" in content
        assert "proxy_set_header Upgrade $http_upgrade;" in content
        assert "proxy_set_header Connection $connection_upgrade;" in content
