use http::Uri;

use super::{classify_control_route, headers};

#[test]
fn classifies_codex_adapter_compatibility_as_admin_global_models_route() {
    let headers = headers(&[]);
    let uri: Uri =
        "/api/admin/models/global/codex-adapter-compatibility?global_model=glm-compatible"
            .parse()
            .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("global_models_manage")
    );
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("codex_adapter_compatibility")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:models")
    );
    assert!(!decision.is_execution_runtime_candidate());
}
