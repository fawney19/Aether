//! Responses WebSocket 的终态观测入口。
//!
//! 这条传输收到的本来就是结构化的 Responses 协议事件。之前为了复用面向 SSE 的
//! `push_line`，观测路径要先把每个事件序列化成 `data: {json}\n\n`，解析器再把它
//! 解码回 `Value`——一次纯粹的往返，而且这个「伪 SSE」形状是随手拼的，一旦
//! 上游事件里出现需要转义的内容，或者以后有人给拼装函数加了换行/分块逻辑，
//! 观测结果就会和真实事件悄悄分叉。
//!
//! 现在观测走 [`StreamingStandardTerminalObserver::push_event`]，直接吃
//! `frame.protocol_events()` 借出的事件，不再序列化、不再解码。
//!
//! **body capture 不走这条路，仍然保持 SSE 形状**（`data: {json}\n\n`）：
//! `aether_usage_runtime::report` 用 `line.strip_prefix("data:")` 解析被捕获的
//! body 来判定 `StreamCapturedTerminalState`，而它是 `stream_report_represents_failure`
//! 的一个 OR 项。把捕获内容换成结构化 JSON 会让终态判定恒为 Missing。
//! 也就是说这一层只换「观测」，不换「捕获」——见
//! [`super::turn::ResponsesProviderAttempt::capture_client_frame`] 一侧仍在用
//! SSE 编码。

use serde_json::Value;

use crate::ai_serving::api::StreamingStandardTerminalObserver;
use aether_contracts::ExecutionStreamTerminalSummary;

/// 包一层 [`StreamingStandardTerminalObserver`]，只暴露结构化入口。
///
/// 存在的意义是让「WS 不再拼 SSE」成为类型层面的事实：这里没有任何接受字节的
/// 方法，所以不可能有人不小心把观测路径改回 `push_line`。
#[derive(Default)]
pub(super) struct ResponsesStructuredTerminalObserver {
    inner: StreamingStandardTerminalObserver,
}

impl ResponsesStructuredTerminalObserver {
    /// 观测一帧里的全部协议事件。
    ///
    /// 第一个被拒绝的事件就停止推进并把摘要标成 parser_error：解析器的状态机是
    /// 有顺序的，跳过一个事件继续喂后面的只会得到更没意义的摘要。
    pub(super) fn observe_events(&mut self, report_context: &Value, events: &[&Value]) {
        for event in events
            .iter()
            .copied()
            .filter(|event| event_is_relevant_to_terminal_observation(event))
        {
            if let Err(error) = self.inner.push_event(report_context, event) {
                self.inner.disable_with_error(error.to_string());
                break;
            }
        }
    }

    pub(super) fn disable_with_error(&mut self, parser_error: impl Into<String>) {
        self.inner.disable_with_error(parser_error);
    }

    pub(super) fn response_id(&self) -> Option<&str> {
        self.inner.latest_summary()?.response_id.as_deref()
    }

    pub(super) fn finish(&mut self, report_context: &Value) -> ExecutionStreamTerminalSummary {
        match self.inner.finish(report_context) {
            Ok(Some(summary)) => summary,
            Ok(None) => ExecutionStreamTerminalSummary::default(),
            Err(error) => {
                self.inner.disable_with_error(error.to_string());
                self.inner.latest_summary().cloned().unwrap_or_default()
            }
        }
    }
}

/// The WebSocket relay is not a Responses schema gateway. It forwards all
/// events opaquely, while this observer consumes only identity/terminal
/// snapshots needed for usage and settlement. In particular, a future
/// `response.*` delta must not become an observation failure merely because
/// Aether's canonical streaming parser does not know it yet.
fn event_is_relevant_to_terminal_observation(event: &Value) -> bool {
    matches!(
        event.get("type").and_then(Value::as_str),
        Some(
            "response.created"
                | "response.in_progress"
                | "response.queued"
                | "response.completed"
                | "response.done"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "error"
        )
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ResponsesStructuredTerminalObserver;

    fn report_context() -> serde_json::Value {
        json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
        })
    }

    #[test]
    fn structured_events_reach_the_terminal_summary_without_sse_text() {
        let context = report_context();
        let created = json!({"type": "response.created", "response": {"id": "resp_ws", "model": "gpt-5-codex"}});
        let completed = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_ws",
                "model": "gpt-5-codex",
                "status": "completed",
                "usage": {"input_tokens": 9, "output_tokens": 4, "total_tokens": 13},
            },
        });

        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.observe_events(&context, &[&created, &completed]);
        let summary = observer.finish(&context);

        assert!(summary.observed_finish);
        assert_eq!(summary.response_id.as_deref(), Some("resp_ws"));
        let usage = summary
            .standardized_usage
            .as_ref()
            .expect("a completed response carries usage");
        assert_eq!(usage.input_tokens, 9);
        assert_eq!(usage.output_tokens, 4);
        assert!(summary.parser_error.is_none());
    }

    /// 批量帧里的多个事件按顺序喂入，usage 不能因为批量而丢失。
    #[test]
    fn a_batched_frame_keeps_the_usage_of_its_last_event() {
        let context = report_context();
        let events = [
            json!({"type": "response.created", "response": {"id": "resp_ws", "model": "m"}}),
            json!({
                "type": "response.output_text.delta",
                "item_id": "msg",
                "output_index": 0,
                "content_index": 0,
                "delta": "hi",
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_ws",
                    "model": "m",
                    "status": "completed",
                    "usage": {"input_tokens": 3, "output_tokens": 1, "total_tokens": 4},
                },
            }),
        ];
        let borrowed: Vec<&serde_json::Value> = events.iter().collect();

        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.observe_events(&context, &borrowed);
        let summary = observer.finish(&context);

        let usage = summary
            .standardized_usage
            .as_ref()
            .expect("usage survives batching");
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 1);
        assert_eq!(usage.dimensions.get("total_tokens"), Some(&json!(4)));
    }

    #[test]
    fn future_and_provider_private_events_are_ignored_only_by_the_side_observer() {
        let context = report_context();
        let private = json!({
            "type": "codex.response.metadata",
            "private_future_field": {"shape": "unknown"},
        });
        let future = json!({
            "type": "response.future_capability.delta",
            "future_capability": {"nested": [1, 2, 3]},
        });
        let completed = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_future",
                "model": "future-model",
                "status": "completed",
                "future_response_field": {"also": "unknown"},
                "usage": {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7},
            },
        });

        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.observe_events(&context, &[&private, &future, &completed]);
        let summary = observer.finish(&context);

        assert!(summary.observed_finish);
        assert_eq!(summary.response_id.as_deref(), Some("resp_future"));
        assert_eq!(summary.unknown_event_count, 0);
        assert_eq!(
            summary
                .standardized_usage
                .as_ref()
                .map(|usage| (usage.input_tokens, usage.output_tokens)),
            Some((5, 2))
        );
        assert!(summary.parser_error.is_none());
    }

    #[test]
    fn a_disabled_observer_reports_the_parser_error() {
        let context = report_context();
        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.disable_with_error("upstream event was not valid JSON");
        let summary = observer.finish(&context);
        assert_eq!(
            summary.parser_error.as_deref(),
            Some("upstream event was not valid JSON")
        );
    }
}
