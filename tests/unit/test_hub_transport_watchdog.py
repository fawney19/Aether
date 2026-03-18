from src.services.proxy_node.hub_config import HubConfig


def test_local_relay_url_uses_http_path() -> None:
    config = HubConfig(
        enabled=True,
        url="http://127.0.0.1:8085",
        connect_timeout_seconds=1.0,
    )

    assert (
        config.local_relay_url("node a/1")
        == "http://127.0.0.1:8085/local/relay/node%20a%2F1"
    )
