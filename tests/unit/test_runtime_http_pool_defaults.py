import pytest

from src.config.settings import Config


def test_config_defaults_to_single_worker_and_capped_http_pool(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for key in (
        "WEB_CONCURRENCY",
        "GUNICORN_WORKERS",
        "HTTP_MAX_CONNECTIONS",
        "HTTP_KEEPALIVE_CONNECTIONS",
    ):
        monkeypatch.delenv(key, raising=False)

    cfg = Config()

    assert cfg.worker_processes == 1
    assert cfg.http_max_connections == 200
    assert cfg.http_keepalive_connections == 60


def test_config_scales_http_pool_down_for_multi_worker(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("GUNICORN_WORKERS", "2")
    monkeypatch.delenv("WEB_CONCURRENCY", raising=False)
    monkeypatch.delenv("HTTP_MAX_CONNECTIONS", raising=False)
    monkeypatch.delenv("HTTP_KEEPALIVE_CONNECTIONS", raising=False)

    cfg = Config()

    assert cfg.worker_processes == 2
    assert cfg.http_max_connections == 100
    assert cfg.http_keepalive_connections == 30
