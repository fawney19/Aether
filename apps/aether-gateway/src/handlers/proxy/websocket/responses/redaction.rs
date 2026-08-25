//! Responses WebSocket 两侧的 PII 脱敏：请求侧 mask + 响应侧 restore。
//!
//! HTTP 路径在前门建 `RedactionSessionSlot` 并塞进 `parts.extensions`，planner
//! 只有拿到这个 slot 才会脱敏。WS 的 planning Parts 是合成的：四个规划入口
//! （首轮、换模型 re-plan、独立轮、配额透明重试）靠 `build_planning_parts` 注入
//! slot 就能复用 planner 的脱敏；但复用已绑定 upstream 的 continuation 根本不进
//! planner，必须在这里先把客户端事件脱敏，再交给协议归一化、上游发送和审计。
//!
//! 因此约定：**进入任何下游用途之前，客户端 `response.create` 只在这里脱敏一次**，
//! 之后所有路径都只看脱敏后的事件。
//!
//! # 响应侧
//!
//! 只 mask 不 restore 是半个实现：HTTP 在把响应交给客户端之前会把占位符换回真实值
//! （`privacy::restore_sync_response_body` / `privacy::StreamingResponseRestorer`），
//! WS 少了这一步，客户端就会直接看到 `<AETHER:EMAIL:...>`。
//! [`ResponsesWebSocketRedactionRestorer`] 补上这一跳，语义与 HTTP 完全一致：
//! 复用 `privacy::restore_json_strings`，只还原本连接自己 mask 出来的映射，
//! 未映射的占位符原样透传。
//!
//! ## session 为什么活在连接上而不是活在这一轮里
//!
//! mask session 由 planner 写进 per-turn 的 slot，而 slot 随 planning Parts 在
//! 规划结束时就被丢弃，响应帧到达时已经无处可取。可选的存活范围有两个：
//!
//! * 挂在 `LogicalTurn` 上：这一轮结束即释放，是 HTTP「一个请求一个 session」的
//!   直译。但 WS 的会话历史留在上游：continuation 只发增量输入，第 1 轮的
//!   `input` 不会在第 3 轮重发。于是第 3 轮的响应里若回显了第 1 轮的占位符
//!   （"你刚才给我的邮箱是……"），本轮 session 里没有这条映射，占位符就漏给客户端。
//!   HTTP 不会漏，是因为它每次都重发整段历史，重新 mask 同一个值会派生出同一个
//!   sentinel（HMAC over 规则 + bucket + 值），所以映射天然齐备。
//! * 挂在当前 response chain 上（当前实现）：每轮仍然各自 mask、各自持有独立 session
//!   （per-turn 语义不变），连接只是把最近若干轮的 session 留下来一起参与还原，
//!   凑出的映射集合正好等于「等价 HTTP 请求会拥有的那一份」。
//!
//! 选后者。省略（或置空）`previous_response_id` 会开始一条独立 response chain，
//! 此时必须丢弃旧链的映射，否则新链里偶然出现的旧 sentinel 会被还原成旧链 PII。
//! 代价是每帧最多对 [`MAX_RETAINED_TURN_REDACTION_SESSIONS`] 个 session 各扫一遍，
//! 以及这些 session 的映射会驻留到当前链结束；用有界 FIFO 兜住上限。
//! 窗口不够用或每帧成本变高时，正确的下一步是在 `privacy` 侧提供跨 session 的
//! 合并匹配器，而不是把这个窗口调大。

use std::collections::VecDeque;

use serde_json::Value;

use crate::ai_serving::{
    resolve_local_decision_execution_runtime_auth_context, resolve_provider_chat_pii_redaction,
};
use crate::control::GatewayControlDecision;
use crate::privacy::{restore_json_strings, RedactionSession, RedactionSessionSlot};
use crate::{AppState, GatewayError};

/// Responses WebSocket 只承载 `openai:responses`，脱敏规则按这个客户端格式选取。
const RESPONSES_WEBSOCKET_CLIENT_API_FORMAT: &str = "openai:responses";

/// WS 在选出候选之前就要脱敏，所以脱敏 session 先记在这个固定 key 下。
///
/// slot 是 per-turn 的（见 `build_planning_parts`），这一轮之后即随 slot 一起丢弃；
/// planner 后续用真实 candidate_id 再取一次配置时，body 已是脱敏态、不会重复写入。
const WEBSOCKET_TURN_REDACTION_CANDIDATE_ID: &str = "responses_websocket_turn";

/// 一条连接最多留几轮的 mask session 用于响应侧还原。
///
/// 取值权衡见模块文档：调大会线性增加每帧还原成本和常驻映射量，调小则更容易漏还原
/// 上游历史里更早那几轮的占位符。8 覆盖的是「上游最可能回显的最近窗口」。
const MAX_RETAINED_TURN_REDACTION_SESSIONS: usize = 8;

/// 一轮客户端 `response.create` 的请求侧脱敏结果。
#[derive(Debug)]
pub(super) struct ResponsesWebSocketTurnRedaction {
    /// 脱敏后的客户端事件；这一轮之后所有下游路径都只看它。
    pub(super) client_event: Value,
    /// 这一轮 mask 出来的映射表，响应侧还原只能靠它。
    pub(super) session: RedactionSession,
}

/// 对一条客户端 `response.create` 做请求侧脱敏。
///
/// 返回 `Some(..)` 仅当脱敏真正命中；`None` 表示未启用或没有命中，调用方
/// 继续用原事件即可（避免未开启脱敏时多一次整包 clone）。
///
/// 脱敏只改写 `instructions` / `input`（见 `privacy::mask_openai_responses_request_value`），
/// `type` / `model` / `previous_response_id` / `generate` 等协议字段原样保留，所以脱敏后的
/// 事件仍可直接用于协议归一化和上游发送。
///
/// 出错必须让这一轮失败：脱敏已启用却读不到配置或加密密钥时，把原文发上游就是
/// 静默旁路，正是本次要修的问题。
pub(super) async fn redact_responses_websocket_client_event(
    state: &AppState,
    parts: &http::request::Parts,
    control_decision: &GatewayControlDecision,
    client_event: &Value,
) -> Result<Option<ResponsesWebSocketTurnRedaction>, GatewayError> {
    redact_responses_websocket_client_event_with_reasoning_replay_policy(
        state,
        parts,
        control_decision,
        client_event,
        crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds,
    )
    .await
}

/// Variant used only after the gateway has selected and authenticated the
/// provider binding. The replay policy comes from that trusted binding, never
/// from client JSON, so a forged reasoning-item shape cannot opt itself into
/// byte-opaque PII handling.
pub(super) async fn redact_responses_websocket_client_event_with_reasoning_replay_policy(
    state: &AppState,
    parts: &http::request::Parts,
    control_decision: &GatewayControlDecision,
    client_event: &Value,
    reasoning_replay_policy: crate::ai_serving::OpenAiResponsesReasoningReplayPolicy,
) -> Result<Option<ResponsesWebSocketTurnRedaction>, GatewayError> {
    let Some(auth_context) =
        resolve_local_decision_execution_runtime_auth_context(control_decision)
    else {
        return Ok(None);
    };
    let redaction = resolve_provider_chat_pii_redaction(
        state,
        parts,
        client_event,
        &auth_context,
        RESPONSES_WEBSOCKET_CLIENT_API_FORMAT,
        reasoning_replay_policy,
        WEBSOCKET_TURN_REDACTION_CANDIDATE_ID,
    )
    .await?;
    if !redaction.redacted {
        return Ok(None);
    }
    // mask 命中时 `resolve_provider_chat_pii_redaction` 必定把 session 写进 slot。
    // 取不到就是内部契约被破坏了，此时继续下发意味着这一轮的响应无法还原、占位符
    // 会漏给客户端；按本模块既有的「脱敏链路出错就让这一轮失败」处理，不做降级。
    let Some(session) = parts
        .extensions
        .get::<RedactionSessionSlot>()
        .and_then(|slot| slot.take_for_candidate(Some(WEBSOCKET_TURN_REDACTION_CANDIDATE_ID)))
    else {
        return Err(GatewayError::Internal(
            "chat pii redaction masked a Responses WebSocket turn without retaining its session"
                .to_string(),
        ));
    };
    Ok(Some(ResponsesWebSocketTurnRedaction {
        client_event: redaction.body_json.into_owned(),
        session,
    }))
}

/// 当前 response chain 上「我们 mask 过哪些映射」的留存集合，供响应侧还原使用。
///
/// 每轮一个独立 session（per-turn mask 语义不变），当前链按 FIFO 留最近
/// [`MAX_RETAINED_TURN_REDACTION_SESSIONS`] 轮。物理上游重绑本身不决定生命周期；
/// `previous_response_id` 决定是否延续旧链。独立请求成功发出时由调用方通过
/// [`Self::start_new_chain`] 原子替换为新链的首轮 session。
#[derive(Default)]
pub(super) struct ResponsesWebSocketRedactionRestorer {
    sessions: VecDeque<RedactionSession>,
}

impl ResponsesWebSocketRedactionRestorer {
    pub(super) fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    /// 登记这一轮的 mask session。
    pub(super) fn register(&mut self, session: RedactionSession) {
        if session.mapping_count() == 0 {
            return;
        }
        self.sessions.push_back(session);
        while self.sessions.len() > MAX_RETAINED_TURN_REDACTION_SESSIONS {
            self.sessions.pop_front();
        }
    }

    /// Commits a successfully started independent response chain.
    ///
    /// Keep this transition next to the successful upstream send/bind. A
    /// rejected independent request has not replaced the active chain and
    /// therefore must not discard the old chain's restore mappings.
    pub(super) fn start_new_chain(&mut self, session: Option<RedactionSession>) {
        self.sessions.clear();
        if let Some(session) = session {
            self.register(session);
        }
    }

    /// 把一帧 provider 事件里的占位符换回真实值，返回要发给客户端的帧文本。
    ///
    /// `None` 表示这一帧没有任何东西要还原，调用方必须原样转发上游字节：未启用
    /// 脱敏（没有任何 session）时连 clone 都不做。
    ///
    /// 入参只读：审计与终态观测继续消费脱敏态的事件，还原只作用于发往客户端的
    /// 那一份拷贝，和 HTTP 侧「审计存脱敏体、线上还原」保持一致。
    pub(super) fn restore_provider_frame_text(&self, event: &Value) -> Option<String> {
        if self.sessions.is_empty() {
            return None;
        }
        let mut restored_event = event.clone();
        let mut restored = false;
        for session in &self.sessions {
            // 逐 session 还原而不是合并映射：每个 session 只认自己 mask 过的
            // sentinel（`RedactionSession::restore_text`），跨 session 合并会绕开
            // 这条边界。同一个值在不同轮派生出的 sentinel 相同，所以顺序无关。
            restored |= restore_json_strings(&mut restored_event, session);
        }
        if !restored {
            return None;
        }
        // 刚从 JSON 解析出来的 Value 再序列化不会失败；真失败时宁可让客户端看到
        // 占位符，也不能丢掉这一帧——丢帧会让客户端的协议状态机卡死。
        serde_json::to_string(&restored_event).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::auth::{
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
    };
    use axum::http::{HeaderMap, Uri};
    use serde_json::{json, Value};

    use super::super::request::{
        build_planning_parts, normalize_followup_response_create, planned_response_create_event,
    };
    use super::super::turn::prepare_responses_websocket_turn_decision;
    use super::super::turn_state::LogicalTurn;
    use super::{
        redact_responses_websocket_client_event, ResponsesWebSocketRedactionRestorer,
        ResponsesWebSocketTurnRedaction, MAX_RETAINED_TURN_REDACTION_SESSIONS,
    };
    use crate::ai_serving::{AiExecutionDecision, ResponsesWebSocketBodyNormalization};
    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
    use crate::handlers::proxy::websocket::ingress::WebSocketRequestContext;
    use crate::AppState;

    const TEST_USER_ID: &str = "user-responses-ws-redaction";
    const TEST_API_KEY_ID: &str = "api-key-responses-ws-redaction";
    const TEST_EMAIL: &str = "ws.user@example.com";
    /// 另一轮用的 PII，用来证明连接级还原覆盖到更早的轮次。
    const OTHER_TEST_EMAIL: &str = "ws.other@example.com";
    /// 不是本连接 mask 出来的占位符：格式合法（符合 sentinel 正则），但没有任何
    /// session 记过它，必须原样透传。
    const FOREIGN_SENTINEL: &str = "<AETHER:EMAIL:AAAAAAAAAAAAAAAAAAAA>";

    fn auth_export_record() -> StoredAuthApiKeyExportRecord {
        StoredAuthApiKeyExportRecord::new(
            TEST_USER_ID.to_string(),
            TEST_API_KEY_ID.to_string(),
            "hash-responses-ws-redaction".to_string(),
            None,
            Some("ws".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            false,
            0,
            0,
            0.0,
            false,
        )
        .expect("auth api key export record should build")
        .with_feature_settings(Some(json!({
            "chat_pii_redaction": {"enabled": true}
        })))
    }

    /// 只装脱敏真正需要的东西：系统配置开关 + 规则、加密密钥、带 feature settings
    /// 的 API Key 导出记录。候选/上游都不需要，这条链路在 planner 之前。
    fn redaction_enabled_state() -> AppState {
        let auth_repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed(vec![])
                .with_export_records(vec![auth_export_record()]),
        );
        let data_state =
            crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                .with_system_config_values_for_tests(vec![
                    ("module.chat_pii_redaction.enabled".to_string(), json!(true)),
                    (
                        "module.chat_pii_redaction.rules".to_string(),
                        json!([{
                            "id": "email",
                            "name": "邮箱",
                            "pattern": r"(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\.[A-Z]{2,63}",
                            "enabled": true,
                            "features": {"validator": "email"},
                            "system": true
                        }]),
                    ),
                    (
                        "module.chat_pii_redaction.cache_ttl_seconds".to_string(),
                        json!(300),
                    ),
                ]);
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state)
    }

    fn control_decision() -> GatewayControlDecision {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/responses".to_string(),
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("responses_websocket".to_string()),
            Some("openai:responses".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: TEST_USER_ID.to_string(),
            api_key_id: TEST_API_KEY_ID.to_string(),
            username: Some("ws".to_string()),
            api_key_name: Some("ws".to_string()),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        });
        decision
    }

    fn websocket_context(decision: GatewayControlDecision) -> WebSocketRequestContext {
        WebSocketRequestContext {
            trace_id: "trace-responses-ws-redaction".to_string(),
            headers: HeaderMap::new(),
            uri: Uri::from_static("/v1/responses"),
            remote_addr: "127.0.0.1:65000"
                .parse::<SocketAddr>()
                .expect("remote address should parse"),
            client_ip: "127.0.0.1".parse().expect("client IP should parse"),
            decision,
            websocket_connection_permit: None,
        }
    }

    fn client_event() -> Value {
        client_event_with_email(TEST_EMAIL)
    }

    fn client_event_with_email(email: &str) -> Value {
        json!({
            "type": "response.create",
            "model": "public-model",
            "previous_response_id": "resp-previous",
            "generate": false,
            "input": [{
                "role": "user",
                "content": [{"type": "input_text", "text": format!("mail {email}")}]
            }]
        })
    }

    /// 真跑一遍请求侧脱敏，拿到这一轮的生效事件和 mask session。
    async fn turn_redaction(
        state: &AppState,
        decision: &GatewayControlDecision,
        email: &str,
    ) -> ResponsesWebSocketTurnRedaction {
        let context = websocket_context(decision.clone());
        let parts = build_planning_parts(&context);
        let event = client_event_with_email(email);
        redact_responses_websocket_client_event(state, &parts, &context.decision, &event)
            .await
            .expect("redaction should resolve")
            .expect("an email in the request should be redacted")
    }

    /// 这一轮为 `email` 派生出的占位符。
    fn sentinel_for(redaction: &ResponsesWebSocketTurnRedaction, email: &str) -> String {
        redaction
            .session
            .sentinel_for_original(email)
            .expect("a masked email must have a sentinel")
            .to_string()
    }

    /// 上游回显占位符的一帧 provider 事件。
    fn provider_delta_frame(text: &str) -> Value {
        json!({
            "type": "response.output_text.delta",
            "item_id": "msg_ws",
            "output_index": 0,
            "content_index": 0,
            "delta": text,
        })
    }

    #[tokio::test]
    async fn websocket_client_event_is_redacted_without_losing_protocol_fields() {
        let state = redaction_enabled_state();
        let context = websocket_context(control_decision());
        let parts = build_planning_parts(&context);
        let event = client_event();

        let redacted =
            redact_responses_websocket_client_event(&state, &parts, &context.decision, &event)
                .await
                .expect("redaction should resolve")
                .expect("an email in the request should be redacted")
                .client_event;

        let serialized = serde_json::to_string(&redacted).expect("event should serialize");
        assert!(!serialized.contains(TEST_EMAIL), "{serialized}");
        assert!(serialized.contains("<AETHER:EMAIL:"), "{serialized}");
        // 协议字段必须原样保留，否则 continuation 链路会断。
        assert_eq!(redacted["type"], "response.create");
        assert_eq!(redacted["model"], "public-model");
        assert_eq!(redacted["previous_response_id"], "resp-previous");
        assert_eq!(redacted["generate"], false);
    }

    #[tokio::test]
    async fn redacting_an_already_redacted_event_is_a_no_op() {
        // re-plan 与配额重试路径会把已脱敏的事件再交给 planner，planner 内部会对
        // 同一个 body 再跑一遍 mask。占位符本身不该被任何规则命中，否则会被二次
        // 替换、破坏与上游已有 previous_response_id 链的一致性。
        let state = redaction_enabled_state();
        let context = websocket_context(control_decision());
        let parts = build_planning_parts(&context);
        let event = client_event();

        let redacted =
            redact_responses_websocket_client_event(&state, &parts, &context.decision, &event)
                .await
                .expect("redaction should resolve")
                .expect("an email in the request should be redacted")
                .client_event;

        // 复用同一个 parts/slot，和 re-plan 在同一 turn 内二次脱敏的情形一致。
        let second_pass =
            redact_responses_websocket_client_event(&state, &parts, &context.decision, &redacted)
                .await
                .expect("second redaction pass should resolve");

        assert!(
            second_pass.is_none(),
            "already redacted event should stay byte-identical: {second_pass:?}"
        );
    }

    #[tokio::test]
    async fn redaction_is_skipped_without_a_local_auth_context() {
        let state = redaction_enabled_state();
        let mut decision = control_decision();
        decision.auth_context = None;
        let context = websocket_context(decision);
        let parts = build_planning_parts(&context);
        let event = client_event();

        let redacted =
            redact_responses_websocket_client_event(&state, &parts, &context.decision, &event)
                .await
                .expect("redaction should resolve");

        assert!(redacted.is_none());
    }

    /// 真跑一遍脱敏，拿到这一轮的「生效事件」。
    async fn redacted_client_event(state: &AppState, decision: &GatewayControlDecision) -> Value {
        turn_redaction(state, decision, TEST_EMAIL)
            .await
            .client_event
    }

    /// 只有 `action` 没有 serde 默认值，其余字段都能省略。
    fn decision_template(
        provider_request_body: Value,
        report_context: Value,
    ) -> AiExecutionDecision {
        serde_json::from_value(json!({
            "action": "local",
            "candidate_id": "candidate-responses-ws",
            "provider_request_body": provider_request_body,
            "report_context": report_context,
        }))
        .expect("decision template should deserialize")
    }

    /// planner 在脱敏 body 上做模型映射后的 provider body。
    fn provider_body_from(effective_event: &Value) -> Value {
        let mut provider_body = effective_event.clone();
        provider_body["model"] = json!("provider-model");
        provider_body
    }

    /// 绑定那一轮留下的 report_context seed：故意带上原始 PII，用来证明这一轮
    /// 会用脱敏后的 body 覆盖它，而不是把原文带进审计。
    fn seed_report_context_with_raw_pii() -> Value {
        json!({
            "request_id": "connection",
            "candidate_id": "candidate-responses-ws",
            "original_request_body": {
                "type": "response.create",
                "model": "public-model",
                "input": format!("mail {TEST_EMAIL}")
            }
        })
    }

    fn assert_redacted_json(value: &Value, label: &str) {
        let serialized = serde_json::to_string(value).expect("value should serialize");
        assert!(
            !serialized.contains(TEST_EMAIL),
            "{label} must not carry raw PII: {serialized}"
        );
        assert!(
            serialized.contains("<AETHER:EMAIL:"),
            "{label} must carry the redaction sentinel: {serialized}"
        );
    }

    #[tokio::test]
    async fn first_turn_upstream_and_audit_bodies_are_redacted() {
        let state = redaction_enabled_state();
        let decision = control_decision();
        let effective_event = redacted_client_event(&state, &decision).await;
        let template = decision_template(
            provider_body_from(&effective_event),
            seed_report_context_with_raw_pii(),
        );
        // 首轮实际发上游的事件由 decision.provider_request_body 派生。
        let normalization = ResponsesWebSocketBodyNormalization::for_tests("provider-model");
        let provider_event: Value = serde_json::from_str(
            &planned_response_create_event(&template, &normalization, &effective_event)
                .expect("first provider event should serialize"),
        )
        .expect("first provider event should parse");

        let turn_decision = prepare_responses_websocket_turn_decision(
            &template,
            "turn-1".to_string(),
            true,
            &effective_event,
            &provider_event,
            "connection",
            1,
            "logical-turn-1",
            1,
        );

        assert_redacted_json(&provider_event, "first turn upstream event");
        assert_redacted_json(
            turn_decision
                .provider_request_body
                .as_ref()
                .expect("turn decision should carry a provider body"),
            "first turn provider request body",
        );
        let report_context = turn_decision
            .report_context
            .as_ref()
            .expect("turn decision should carry a report context");
        assert_redacted_json(
            &report_context["original_request_body"],
            "first turn audit body",
        );
        // 整个 report_context 都不该残留原文（seed 里的原始 body 必须被覆盖）。
        assert_redacted_json(report_context, "first turn report context");
        assert_eq!(provider_event["type"], "response.create");
        assert_eq!(provider_event["model"], "provider-model");
    }

    #[tokio::test]
    async fn continuation_upstream_and_audit_bodies_are_redacted() {
        let state = redaction_enabled_state();
        let decision = control_decision();
        let effective_event = redacted_client_event(&state, &decision).await;
        // continuation 复用已绑定的 upstream：不再规划，直接重放归一化器。
        let outbound = normalize_followup_response_create(
            &effective_event,
            "provider-model",
            &ResponsesWebSocketBodyNormalization::for_tests("provider-model"),
        )
        .expect("continuation should normalize");
        let provider_event: Value =
            serde_json::from_str(&outbound).expect("continuation event should parse");

        let template = decision_template(
            provider_body_from(&effective_event),
            seed_report_context_with_raw_pii(),
        );
        let turn_decision = prepare_responses_websocket_turn_decision(
            &template,
            "turn-2".to_string(),
            false,
            &effective_event,
            &provider_event,
            "connection",
            2,
            "logical-turn-2",
            1,
        );

        assert!(
            !outbound.contains(TEST_EMAIL),
            "continuation upstream frame must not carry raw PII: {outbound}"
        );
        assert!(
            outbound.contains("<AETHER:EMAIL:"),
            "continuation upstream frame must carry the sentinel: {outbound}"
        );
        assert_eq!(provider_event["previous_response_id"], "resp-previous");
        let report_context = turn_decision
            .report_context
            .as_ref()
            .expect("turn decision should carry a report context");
        assert_redacted_json(
            &report_context["original_request_body"],
            "continuation audit body",
        );
        assert_redacted_json(report_context, "continuation report context");
    }

    #[tokio::test]
    async fn quota_retry_replays_the_redacted_event() {
        let state = redaction_enabled_state();
        let decision = control_decision();
        let effective_event = redacted_client_event(&state, &decision).await;
        // 配额透明重试重放 LogicalTurn 里保存的事件，所以保存的必须
        // 已经是脱敏版，否则重试会把原文发给新的上游账号。
        let active = LogicalTurn::new(effective_event.clone(), 2, "logical-turn-2".to_string());
        assert_redacted_json(&active.client_event, "quota retry replay event");

        let template = decision_template(
            provider_body_from(&active.client_event),
            seed_report_context_with_raw_pii(),
        );
        let normalization = ResponsesWebSocketBodyNormalization::for_tests("provider-model");
        let provider_event: Value = serde_json::from_str(
            &planned_response_create_event(&template, &normalization, &active.client_event)
                .expect("retry provider event should serialize"),
        )
        .expect("retry provider event should parse");
        let turn_decision = prepare_responses_websocket_turn_decision(
            &template,
            "turn-2-retry".to_string(),
            true,
            &active.client_event,
            &provider_event,
            "connection",
            active.turn_index,
            "logical-turn-2",
            2,
        );

        assert_redacted_json(&provider_event, "quota retry upstream event");
        let report_context = turn_decision
            .report_context
            .as_ref()
            .expect("turn decision should carry a report context");
        assert_eq!(report_context["websocket_turn_attempt"], 2);
        assert_redacted_json(
            &report_context["original_request_body"],
            "quota retry audit body",
        );
        assert_redacted_json(report_context, "quota retry report context");
    }

    // -----------------------------------------------------------------------
    // 响应侧还原
    // -----------------------------------------------------------------------

    /// 本次修复的核心：上游把占位符回显在事件里，客户端必须拿到真实值。
    #[tokio::test]
    async fn provider_frame_placeholders_are_restored_before_client_delivery() {
        let state = redaction_enabled_state();
        let redaction = turn_redaction(&state, &control_decision(), TEST_EMAIL).await;
        let sentinel = sentinel_for(&redaction, TEST_EMAIL);
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(redaction.session);

        let frame = provider_delta_frame(&format!("your mail is {sentinel}"));
        let restored = restorer
            .restore_provider_frame_text(&frame)
            .expect("a frame echoing this turn's sentinel must be restored");

        assert!(
            restored.contains(TEST_EMAIL),
            "the client must receive the real value: {restored}"
        );
        assert!(
            !restored.contains(&sentinel),
            "no sentinel may survive to the client: {restored}"
        );
        // 协议字段不受影响，客户端的状态机照旧。
        let restored: Value = serde_json::from_str(&restored).expect("restored frame is JSON");
        assert_eq!(restored["type"], "response.output_text.delta");
        assert_eq!(restored["item_id"], "msg_ws");
        assert_eq!(restored["output_index"], 0);
    }

    /// Codex 把多个事件批量塞进 `{"chunks":[...]}`，还原必须走进批量里。
    #[tokio::test]
    async fn placeholders_batched_inside_a_chunks_envelope_are_restored() {
        let state = redaction_enabled_state();
        let redaction = turn_redaction(&state, &control_decision(), TEST_EMAIL).await;
        let sentinel = sentinel_for(&redaction, TEST_EMAIL);
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(redaction.session);

        let frame = json!({
            "chunks": [
                provider_delta_frame("plain delta"),
                provider_delta_frame(&format!("mail {sentinel}")),
            ]
        });
        let restored = restorer
            .restore_provider_frame_text(&frame)
            .expect("a batched sentinel must be restored");

        assert!(restored.contains(TEST_EMAIL), "{restored}");
        assert!(!restored.contains(&sentinel), "{restored}");
    }

    /// 只还原本连接 mask 过的映射，和 `RedactionSession::restore_text` 一致：
    /// 别处来的占位符（比如客户端自己发的、或上一条连接的）保持原样。
    #[tokio::test]
    async fn an_unmapped_placeholder_is_left_untouched() {
        let state = redaction_enabled_state();
        let redaction = turn_redaction(&state, &control_decision(), TEST_EMAIL).await;
        let sentinel = sentinel_for(&redaction, TEST_EMAIL);
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(redaction.session);

        let frame = provider_delta_frame(&format!("{FOREIGN_SENTINEL} and {sentinel}"));
        let restored = restorer
            .restore_provider_frame_text(&frame)
            .expect("the mapped sentinel is still restored");

        assert!(restored.contains(TEST_EMAIL), "{restored}");
        assert!(
            restored.contains(FOREIGN_SENTINEL),
            "an unmapped placeholder must survive verbatim: {restored}"
        );
    }

    /// 没有命中还原时必须让调用方原样转发上游字节。
    #[tokio::test]
    async fn a_frame_without_known_placeholders_is_not_rewritten() {
        let state = redaction_enabled_state();
        let redaction = turn_redaction(&state, &control_decision(), TEST_EMAIL).await;
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(redaction.session);

        assert!(
            restorer
                .restore_provider_frame_text(&provider_delta_frame("nothing to restore"))
                .is_none(),
            "a frame with no mapped sentinel must be relayed byte-for-byte"
        );
        assert!(
            restorer
                .restore_provider_frame_text(&provider_delta_frame(FOREIGN_SENTINEL))
                .is_none(),
            "a frame that only carries unmapped placeholders must not be rewritten"
        );
    }

    /// 未启用脱敏（或这条连接从没 mask 到东西）时，还原器必须完全不介入：
    /// 连 clone 都不做，输出就是上游原字节。
    #[tokio::test]
    async fn a_restorer_without_sessions_never_rewrites_a_frame() {
        let restorer = ResponsesWebSocketRedactionRestorer::default();

        assert!(restorer
            .restore_provider_frame_text(&provider_delta_frame(FOREIGN_SENTINEL))
            .is_none());
        assert!(restorer
            .restore_provider_frame_text(&provider_delta_frame(TEST_EMAIL))
            .is_none());
    }

    /// 空 session（启用了脱敏但这一轮没命中任何规则）不该被留下来白扫每一帧。
    #[tokio::test]
    async fn a_session_without_mappings_is_not_retained() {
        let state = redaction_enabled_state();
        let hmac_key = state
            .encryption_key()
            .expect("the test state carries an encryption key")
            .as_bytes()
            .to_vec();
        let empty_session = crate::privacy::RedactionSession::new(
            crate::privacy::RedactionSessionConfig::default_ttl(hmac_key, 0),
        );
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(empty_session);

        assert!(restorer
            .restore_provider_frame_text(&provider_delta_frame(FOREIGN_SENTINEL))
            .is_none());
    }

    /// 还原只作用于发给客户端的那一份拷贝：审计和终态观测消费的事件必须保持脱敏态。
    #[tokio::test]
    async fn restoring_does_not_mutate_the_event_the_audit_path_keeps() {
        let state = redaction_enabled_state();
        let redaction = turn_redaction(&state, &control_decision(), TEST_EMAIL).await;
        let sentinel = sentinel_for(&redaction, TEST_EMAIL);
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(redaction.session);

        let frame = provider_delta_frame(&format!("mail {sentinel}"));
        let before = frame.clone();
        let _ = restorer
            .restore_provider_frame_text(&frame)
            .expect("the frame is restored for the client");

        assert_eq!(
            frame, before,
            "capture_client_frame / 终态观测拿到的事件必须仍是脱敏态"
        );
    }

    /// 连接级持有的意义：WS 的会话历史留在上游，continuation 只发增量输入，
    /// 所以第 2 轮的响应可能回显第 1 轮的占位符。per-turn 持有会漏掉这一条。
    #[tokio::test]
    async fn a_later_turn_restores_a_placeholder_first_masked_by_an_earlier_turn() {
        let state = redaction_enabled_state();
        let decision = control_decision();
        let first = turn_redaction(&state, &decision, TEST_EMAIL).await;
        let second = turn_redaction(&state, &decision, OTHER_TEST_EMAIL).await;
        let first_sentinel = sentinel_for(&first, TEST_EMAIL);
        let second_sentinel = sentinel_for(&second, OTHER_TEST_EMAIL);
        assert_ne!(first_sentinel, second_sentinel);

        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(first.session);
        restorer.register(second.session);

        let frame = provider_delta_frame(&format!("{first_sentinel} then {second_sentinel}"));
        let restored = restorer
            .restore_provider_frame_text(&frame)
            .expect("both turns' sentinels are restorable on this connection");

        assert!(restored.contains(TEST_EMAIL), "{restored}");
        assert!(restored.contains(OTHER_TEST_EMAIL), "{restored}");
        assert!(!restored.contains(&first_sentinel), "{restored}");
        assert!(!restored.contains(&second_sentinel), "{restored}");
    }

    /// Omitting `previous_response_id` starts a new response chain. Restore
    /// mappings from the prior chain must not leak into that independent
    /// response, while the new chain's first-turn mapping remains available.
    #[tokio::test]
    async fn an_independent_chain_replaces_prior_restore_mappings() {
        let state = redaction_enabled_state();
        let decision = control_decision();
        let prior = turn_redaction(&state, &decision, TEST_EMAIL).await;
        let current = turn_redaction(&state, &decision, OTHER_TEST_EMAIL).await;
        let prior_sentinel = sentinel_for(&prior, TEST_EMAIL);
        let current_sentinel = sentinel_for(&current, OTHER_TEST_EMAIL);

        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(prior.session);
        restorer.start_new_chain(Some(current.session));

        assert!(
            restorer
                .restore_provider_frame_text(&provider_delta_frame(&prior_sentinel))
                .is_none(),
            "an independent chain must not restore PII from its predecessor"
        );
        let restored = restorer
            .restore_provider_frame_text(&provider_delta_frame(&current_sentinel))
            .expect("the new chain's first-turn mapping must remain available");
        assert!(restored.contains(OTHER_TEST_EMAIL), "{restored}");
        assert!(!restored.contains(&current_sentinel), "{restored}");
    }

    #[tokio::test]
    async fn an_unredacted_independent_chain_clears_prior_restore_mappings() {
        let state = redaction_enabled_state();
        let prior = turn_redaction(&state, &control_decision(), TEST_EMAIL).await;
        let prior_sentinel = sentinel_for(&prior, TEST_EMAIL);

        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(prior.session);
        assert!(restorer.has_sessions());

        restorer.start_new_chain(None);

        assert!(!restorer.has_sessions());
        assert!(restorer
            .restore_provider_frame_text(&provider_delta_frame(&prior_sentinel))
            .is_none());
    }

    /// 留存窗口是有界的：长连接不能无限累积映射，代价是更早的轮次会退回
    /// 「占位符原样透传」而不是被错误还原成别的值。
    #[tokio::test]
    async fn the_retained_session_window_is_bounded() {
        let state = redaction_enabled_state();
        let decision = control_decision();
        let oldest = turn_redaction(&state, &decision, TEST_EMAIL).await;
        let oldest_sentinel = sentinel_for(&oldest, TEST_EMAIL);
        let mut restorer = ResponsesWebSocketRedactionRestorer::default();
        restorer.register(oldest.session);

        // 再灌满整个窗口，最老的那一轮必须被挤出去。
        let mut newest_sentinel = String::new();
        for index in 0..MAX_RETAINED_TURN_REDACTION_SESSIONS {
            let email = format!("ws.turn{index}@example.com");
            let redaction = turn_redaction(&state, &decision, &email).await;
            newest_sentinel = sentinel_for(&redaction, &email);
            restorer.register(redaction.session);
        }

        assert!(
            restorer
                .restore_provider_frame_text(&provider_delta_frame(&oldest_sentinel))
                .is_none(),
            "the evicted turn's sentinel is relayed verbatim, never mis-restored"
        );
        assert!(
            restorer
                .restore_provider_frame_text(&provider_delta_frame(&newest_sentinel))
                .is_some(),
            "the most recent turns stay restorable"
        );
    }
}
