from src.api.admin.pool.routes import _derive_oauth_plan_type
from src.models.database import ProviderAPIKey


def test_derive_oauth_plan_type_prefers_realtime_codex_metadata() -> None:
    key = ProviderAPIKey(
        id="pool-key-1",
        provider_id="provider-1",
        auth_type="oauth",
        upstream_metadata={"codex": {"plan_type": "plus"}},
        name="codex-user",
    )
    key.oauth_plan_type = "free"

    plan_type = _derive_oauth_plan_type(
        key,
        "codex",
        auth_config={"plan_type": "free"},
    )

    assert plan_type == "plus"
