use async_trait::async_trait;

#[async_trait]
pub trait AiAuthenticatedDecisionInputPort: Send + Sync {
    type AuthContext: Send + Sync;
    type AuthSnapshot: Send;
    type RequiredCapabilities: Send + Sync;
    type ResolvedInput: Send;
    type Error: Send;

    async fn read_auth_snapshot(
        &self,
        auth_context: &Self::AuthContext,
    ) -> Result<Option<Self::AuthSnapshot>, Self::Error>;

    async fn resolve_required_capabilities(
        &self,
        auth_context: &Self::AuthContext,
        requested_model: Option<&str>,
        explicit_required_capabilities: Option<&Self::RequiredCapabilities>,
    ) -> Result<Option<Self::RequiredCapabilities>, Self::Error>;

    fn build_resolved_input(
        &self,
        auth_context: Self::AuthContext,
        auth_snapshot: Self::AuthSnapshot,
        required_capabilities: Option<Self::RequiredCapabilities>,
    ) -> Self::ResolvedInput;
}

pub async fn run_ai_authenticated_decision_input<Port>(
    port: &Port,
    auth_context: Port::AuthContext,
    requested_model: Option<&str>,
    explicit_required_capabilities: Option<&Port::RequiredCapabilities>,
) -> Result<Option<Port::ResolvedInput>, Port::Error>
where
    Port: AiAuthenticatedDecisionInputPort,
{
    let auth_snapshot = match port.read_auth_snapshot(&auth_context).await? {
        Some(snapshot) => snapshot,
        None => return Ok(None),
    };

    let required_capabilities = port
        .resolve_required_capabilities(
            &auth_context,
            requested_model,
            explicit_required_capabilities,
        )
        .await?;

    Ok(Some(port.build_resolved_input(
        auth_context,
        auth_snapshot,
        required_capabilities,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestAuthContext {
        user_id: &'static str,
        api_key_id: &'static str,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestResolvedInput {
        auth_context: TestAuthContext,
        auth_snapshot: &'static str,
        required_capabilities: Option<String>,
    }

    struct TestPort {
        auth_snapshot: Option<&'static str>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiAuthenticatedDecisionInputPort for TestPort {
        type AuthContext = TestAuthContext;
        type AuthSnapshot = &'static str;
        type RequiredCapabilities = String;
        type ResolvedInput = TestResolvedInput;
        type Error = std::convert::Infallible;

        async fn read_auth_snapshot(
            &self,
            auth_context: &Self::AuthContext,
        ) -> Result<Option<Self::AuthSnapshot>, Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "snapshot:{}:{}",
                auth_context.user_id, auth_context.api_key_id
            ));
            Ok(self.auth_snapshot)
        }

        async fn resolve_required_capabilities(
            &self,
            auth_context: &Self::AuthContext,
            requested_model: Option<&str>,
            explicit_required_capabilities: Option<&Self::RequiredCapabilities>,
        ) -> Result<Option<Self::RequiredCapabilities>, Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "capabilities:{}:{}:{}",
                auth_context.user_id,
                requested_model.unwrap_or_default(),
                explicit_required_capabilities
                    .map(String::as_str)
                    .unwrap_or_default()
            ));
            Ok(Some("merged-capabilities".to_string()))
        }

        fn build_resolved_input(
            &self,
            auth_context: Self::AuthContext,
            auth_snapshot: Self::AuthSnapshot,
            required_capabilities: Option<Self::RequiredCapabilities>,
        ) -> Self::ResolvedInput {
            TestResolvedInput {
                auth_context,
                auth_snapshot,
                required_capabilities,
            }
        }
    }

    #[tokio::test]
    async fn authenticated_decision_input_resolves_snapshot_and_capabilities() {
        let port = TestPort {
            auth_snapshot: Some("snapshot-a"),
            calls: Mutex::new(Vec::new()),
        };
        let auth_context = TestAuthContext {
            user_id: "user-a",
            api_key_id: "key-a",
        };
        let explicit = "explicit-capability".to_string();

        let resolved = run_ai_authenticated_decision_input(
            &port,
            auth_context.clone(),
            Some("model-a"),
            Some(&explicit),
        )
        .await
        .unwrap();

        assert_eq!(
            resolved,
            Some(TestResolvedInput {
                auth_context,
                auth_snapshot: "snapshot-a",
                required_capabilities: Some("merged-capabilities".to_string()),
            })
        );
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "snapshot:user-a:key-a",
                "capabilities:user-a:model-a:explicit-capability",
            ]
        );
    }

    #[tokio::test]
    async fn authenticated_decision_input_stops_when_snapshot_is_missing() {
        let port = TestPort {
            auth_snapshot: None,
            calls: Mutex::new(Vec::new()),
        };

        let resolved = run_ai_authenticated_decision_input(
            &port,
            TestAuthContext {
                user_id: "user-a",
                api_key_id: "key-a",
            },
            Some("model-a"),
            None,
        )
        .await
        .unwrap();

        assert_eq!(resolved, None);
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["snapshot:user-a:key-a"]
        );
    }
}
