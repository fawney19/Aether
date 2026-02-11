from src.config.constants import CacheTTL


def test_activity_heatmap_ttl_is_two_hours() -> None:
    assert CacheTTL.ACTIVITY_HEATMAP == 7200
