//! 一次 provider attempt 的记账生命周期，与 transport 无关。
//!
//! HTTP 流式与 Responses WebSocket 的记账都是同一个三段结构：
//! `pending` → `started` → `terminal`。差异只在「终态事实从哪来」——HTTP 从
//! SSE 字节流解析，WS 从协议事件观察。在这里抽出来之前，WS 侧在
//! `handlers/proxy/websocket/responses/turn.rs` 里重写了一遍 usage 写入、
//! candidate 状态流转、health/adaptive 效果投射、pool key lease 释放、
//! body capture 和账单失败判定，与 HTTP 的顺序、超时语义只能靠人工对齐。
//!
//! # HTTP 侧调用点映射
//!
//! 本批不改 HTTP 执行路径（`execution_runtime/stream/execution.rs` 里的
//! `DirectPassthroughFinalizerCore` 与 failover / oauth 重试 / prefetch 深度纠缠，
//! 无法在「行为等价 + 单 commit 可验证」的前提下接线）。这里记下逐调用点的对应
//! 关系，作为后续 PR 的接线依据：
//!
//! | HTTP 现状调用点 | 对应本模块 |
//! |---|---|
//! | `record_stream_pending_lifecycle` | [`ExecutionAttemptLifecycle::begin`] |
//! | `maybe_record_first_stream_event_started` | [`ExecutionAttemptLifecycle::mark_started`] |
//! | `record_stream_terminal_usage` | `settle` 第 1 段：usage terminal |
//! | `enqueue_stream_candidate_status_update` | `settle` 第 2 段：candidate terminal |
//! | `apply_local_stream_{success,failure}_effects` | `settle` 第 3 段：provider effects |
//! | `submit_stream_report` | `settle` 第 4 段：execution report |
//! | `append_stream_capture_bytes` / `build_stream_body_capture` | [`AttemptBodyCapture`] |
//!
//! HTTP 侧的 `stage_trace` 埋点、kiro prompt-cache usage 合并、direct-inline 延迟
//! pending 等是它独有的，接线时作为 transport 专有部分留在原处。

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use aether_contracts::{ExecutionPlan, ExecutionStreamTerminalSummary, ExecutionTelemetry};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;
#[cfg(test)]
use aether_usage_runtime::DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES;
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_stream_terminal_usage_payload_seed,
    build_terminal_usage_context_seed, stream_report_represents_failure,
};
use base64::Engine as _;
use serde_json::Value;
use tracing::warn;

use crate::clock::current_unix_ms;
use crate::orchestration::{
    apply_local_stream_failure_effects, apply_local_stream_success_effects,
    release_local_pool_key_lease, LocalExecutionEffectContext, LocalStreamFailureEffect,
};
use crate::request_candidate_runtime::record_local_request_candidate_status;
use crate::usage::{submit_stream_report, GatewayStreamReportRequest};
use crate::AppState;

/// 客户端取消/断开时对外记录的状态码。
pub(crate) const CLIENT_CANCELLED_STATUS_CODE: u16 = 499;

/// 流式超时状态码；只有它会额外投射 pool stream timeout 效果。
pub(crate) const STREAM_TIMEOUT_STATUS_CODE: u16 = 504;

/// provider 侧观察到的终态。
///
/// 形状刻意保持 transport 中立：HTTP 流式与 WS turn 的差异只在事实从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptProviderOutcome {
    /// 观察到了供应商的终态事件。
    Terminal {
        status_code: u16,
        /// 供应商自己声明这一轮被取消（`response.cancelled`）。
        cancelled_by_provider: bool,
    },
    /// 供应商没能给出终态：断链、超时、gateway 侧失败。
    Aborted {
        status_code: u16,
        reason: &'static str,
        stream_timeout: bool,
    },
}

impl AttemptProviderOutcome {
    pub(crate) const fn status_code(self) -> u16 {
        match self {
            Self::Terminal { status_code, .. } | Self::Aborted { status_code, .. } => status_code,
        }
    }

    pub(crate) const fn cancelled_by_provider(self) -> bool {
        matches!(
            self,
            Self::Terminal {
                cancelled_by_provider: true,
                ..
            }
        )
    }

    pub(crate) const fn stream_timeout(self) -> bool {
        matches!(
            self,
            Self::Aborted {
                stream_timeout: true,
                ..
            }
        )
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminal { .. })
    }
}

/// 这一个 attempt 的内容是否完整交付给了客户端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptClientDelivery {
    Complete,
    Aborted { reason: &'static str },
}

impl AttemptClientDelivery {
    pub(crate) const fn aborted_reason(self) -> Option<&'static str> {
        match self {
            Self::Complete => None,
            Self::Aborted { reason } => Some(reason),
        }
    }

    pub(crate) const fn is_aborted(self) -> bool {
        matches!(self, Self::Aborted { .. })
    }
}

/// 一次 attempt 结算时的两个正交事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttemptTerminalFacts {
    pub(crate) provider: AttemptProviderOutcome,
    pub(crate) delivery: AttemptClientDelivery,
}

impl AttemptTerminalFacts {
    /// 记入 usage / candidate / 效果的人类可读原因。
    pub(crate) const fn reason(self) -> &'static str {
        if let Some(reason) = self.delivery.aborted_reason() {
            return reason;
        }
        match self.provider {
            AttemptProviderOutcome::Terminal {
                cancelled_by_provider: true,
                ..
            } => "provider cancelled the response",
            AttemptProviderOutcome::Terminal { .. } => {
                "provider returned a terminal response event"
            }
            AttemptProviderOutcome::Aborted { reason, .. } => reason,
        }
    }

    /// 供应商侧强制错误原因：只有「供应商没给出终态、且内容已完整交付客户端」
    /// 才算，用于给终态摘要补 `parser_error`。
    ///
    /// 客户端投递失败不是供应商的错误，所以那一侧返回 `None`——与现状
    /// `ResponsesWebSocketTurnOutcome::forced_error()` 对 `Cancelled` 返回
    /// `None` 一致。
    pub(crate) const fn forced_error(self) -> Option<&'static str> {
        match (self.provider, self.delivery) {
            (AttemptProviderOutcome::Aborted { reason, .. }, AttemptClientDelivery::Complete) => {
                Some(reason)
            }
            _ => None,
        }
    }
}

/// 这条 usage 记录是否计费。`Void` 等价于现状传给
/// `record_stream_terminal(.., cancelled = true)` 的那一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptBilling {
    Billed,
    Void,
}

impl AttemptBilling {
    pub(crate) const fn is_void(self) -> bool {
        matches!(self, Self::Void)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptCandidateStatus {
    Success,
    Failed,
    Cancelled,
}

/// candidate 行上记录的错误分类。
///
/// 与 [`AttemptCandidateStatus`] 刻意分开：`missing_terminal` 为真而记账层
/// 判定不算失败（report kind 不要求观察到终态事件）时，现状会写出
/// 「状态 Success + error_type=stream_missing_terminal_event」的组合，
/// 这里必须原样保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptCandidateError {
    None,
    Cancelled,
    /// 供应商已经给出终态，但这一轮内容没能完整交付给客户端。账单照记，
    /// candidate 行上留下这条事实。
    ClientDeliveryFailed,
    MissingTerminal,
    TerminalError,
}

/// 一次 attempt 结束后要投射给供应商/密钥池的效果。
///
/// 每个分支都会释放 pool key lease：`ProviderFailure` 由 `PoolError` 释放，
/// `ProviderSuccess` 由 `PoolSuccessStream` 释放，其余情况直接释放。少一条
/// 分支就会把 lease 挂到 TTL 过期，等于短时间占死一把 key。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptProviderEffect {
    /// 既不投射成功也不投射失败，只把 lease 还回去。
    ReleasePoolKeyLease,
    ProviderFailure,
    ProviderSuccess,
}

impl AttemptProviderEffect {
    /// 把「每个分支都必须释放 lease」这条不变量显式化，便于测试锁住
    /// 「没进任何分支导致 lease 泄漏」这类回归。
    const fn releases_pool_key_lease(self) -> bool {
        match self {
            Self::ReleasePoolKeyLease | Self::ProviderFailure | Self::ProviderSuccess => true,
        }
    }
}

/// 判定一次 attempt 结束后要投射的效果。
///
/// 关键分支是「记账层判成 failed，但这一轮没有投射供应商失败」：例如合法的
/// `response.incomplete`（写满 max_output_tokens）。共享 usage 判定目前仍会
/// 把这类终态记成失败，但供应商本身工作正常，既不该扣健康分，也不能因为落
/// 不到任何分支而漏掉 lease 释放。
pub(crate) const fn classify_attempt_provider_effect(
    cancelled: bool,
    projects_provider_failure: bool,
    failed: bool,
) -> AttemptProviderEffect {
    if cancelled {
        AttemptProviderEffect::ReleasePoolKeyLease
    } else if projects_provider_failure {
        AttemptProviderEffect::ProviderFailure
    } else if failed {
        AttemptProviderEffect::ReleasePoolKeyLease
    } else {
        AttemptProviderEffect::ProviderSuccess
    }
}

/// 这一个 attempt 的账单是否作废。
///
/// 只有两种情况作废：供应商自己声明取消，或者供应商根本没给出终态而客户端
/// 又已经走了。**供应商已经给出终态时，客户端最后一跳投递失败不作废账单**：
/// 供应商已经完成推理并消耗了 token，客户端还能用 `previous_response_id`
/// 续取这条响应，把成本记成 0 等于让上游账单凭空消失。
pub(crate) const fn attempt_billing_is_void(facts: AttemptTerminalFacts) -> bool {
    facts.provider.cancelled_by_provider()
        || (facts.delivery.is_aborted() && !facts.provider.is_terminal())
}

/// attempt 对外记录的状态码。
///
/// 状态码现在纯粹是 provider 事实：客户端投递失败不再把一条已经拿到 200
/// 终态的记录改写成 499。作废分支的 provider 状态码本身就是 499
/// （`response.cancelled` 映射 499，`Cancelled` 信号的兜底也是 499），
/// 所以这些行的取值不变。
pub(crate) const fn attempt_status_code(facts: AttemptTerminalFacts) -> u16 {
    facts.provider.status_code()
}

/// 结算判定的输入：两个正交事实 + 记账层对这条 report 的判定 + 终态摘要事实。
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttemptSettlementInputs {
    pub(crate) facts: AttemptTerminalFacts,
    /// `aether_usage_runtime::stream_report_represents_failure(payload)` 的结果。
    pub(crate) report_represents_failure: bool,
    /// 终态摘要里是否观察到了 finish。
    pub(crate) observed_finish: bool,
    /// 终态摘要里是否带解析错误。
    pub(crate) has_parser_error: bool,
}

/// 结算动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttemptSettlement {
    pub(crate) status_code: u16,
    pub(crate) billing: AttemptBilling,
    pub(crate) candidate_status: AttemptCandidateStatus,
    pub(crate) candidate_error: AttemptCandidateError,
    pub(crate) provider_effect: AttemptProviderEffect,
    pub(crate) submit_execution_report: bool,
}

/// 由两个正交事实推出结算动作。唯一的判定入口，表驱动测试逐行锁死。
///
/// provider 终态已到达时，客户端投递失败只影响 candidate 的错误分类，不再作废
/// 账单、不再把状态码改成 499、也不再跳过供应商效果和 execution report。
pub(crate) const fn classify_attempt_settlement(
    inputs: AttemptSettlementInputs,
) -> AttemptSettlement {
    let AttemptSettlementInputs {
        facts,
        report_represents_failure,
        observed_finish,
        has_parser_error,
    } = inputs;

    let void = attempt_billing_is_void(facts);
    let status_code = attempt_status_code(facts);
    let failed = !void && report_represents_failure;
    let missing_terminal = !void && !observed_finish;
    let projects_provider_failure = !void
        && (status_code >= 400
            || facts.forced_error().is_some()
            || has_parser_error
            || missing_terminal);

    let candidate_status = if void {
        AttemptCandidateStatus::Cancelled
    } else if failed {
        AttemptCandidateStatus::Failed
    } else {
        AttemptCandidateStatus::Success
    };
    // 投递失败排在供应商侧分类之前：这条记录之所以特别，正是因为内容没送到
    // 客户端手上。供应商侧的判定仍然通过 candidate_status 和 error_message
    // 保留下来。
    let candidate_error = if void {
        AttemptCandidateError::Cancelled
    } else if facts.delivery.is_aborted() {
        AttemptCandidateError::ClientDeliveryFailed
    } else if missing_terminal {
        AttemptCandidateError::MissingTerminal
    } else if failed {
        AttemptCandidateError::TerminalError
    } else {
        AttemptCandidateError::None
    };

    AttemptSettlement {
        status_code,
        billing: if void {
            AttemptBilling::Void
        } else {
            AttemptBilling::Billed
        },
        candidate_status,
        candidate_error,
        provider_effect: classify_attempt_provider_effect(void, projects_provider_failure, failed),
        submit_execution_report: !void,
    }
}

/// 每一段记账 I/O 的等待上界。
///
/// WS 用 `Bounded(5s)`：relay loop 是单任务，一段慢依赖会拖住整条连接的收发。
/// HTTP 接线时用 `Unbounded` 即保持它现在的语义。
#[derive(Debug, Clone, Copy)]
pub(crate) enum AttemptStageGuard {
    Unbounded,
    Bounded(Duration),
}

impl AttemptStageGuard {
    /// 等一段记账 I/O，超时就放弃等待。
    ///
    /// 返回 `None` 表示这一段没有在上界内完成；调用方据此决定兜底动作
    /// （例如效果段超时后仍然要释放 pool key lease）。
    pub(crate) async fn await_stage<T>(
        self,
        trace_id: &str,
        stage: &'static str,
        future: impl Future<Output = T>,
    ) -> Option<T> {
        let Self::Bounded(timeout) = self else {
            return Some(future.await);
        };
        match tokio::time::timeout(timeout, future).await {
            Ok(value) => Some(value),
            Err(_) => {
                warn!(
                    event_name = "execution_attempt_lifecycle_stage_timeout",
                    log_type = "ops",
                    trace_id,
                    stage,
                    timeout_ms = timeout.as_millis() as u64,
                    "gateway stopped waiting for an execution attempt lifecycle stage"
                );
                None
            }
        }
    }

    /// 跑一段不能丢的写入，同时仍然给调用方的等待设上界。
    ///
    /// [`Self::await_stage`] 超时会 drop 掉它等待的 future，这对次要效果是对的，
    /// 但会静默丢弃系统其余部分依赖的写入。先 spawn 再等，让上界只约束「等多久」：
    /// 丢弃 `JoinHandle` 只是让任务脱离，它仍会跑完。
    pub(crate) async fn await_detachable_stage<F>(
        self,
        trace_id: &str,
        stage: &'static str,
        write: F,
    ) where
        F: Future<Output = ()> + Send + 'static,
    {
        let _ = self.await_stage(trace_id, stage, tokio::spawn(write)).await;
    }
}

/// 一侧（provider 或 client）的响应体捕获缓冲。
///
/// 捕获内容必须保持 SSE 形状（`data: {json}\n\n`）：
/// `aether_usage_runtime` 会按 `data:` 行解析被捕获的 body 来判定
/// `StreamCapturedTerminalState`，而它是 `stream_report_represents_failure`
/// 的一个 OR 项。换成结构化 JSON 会让终态判定恒为 Missing。
#[derive(Debug, Default)]
pub(crate) struct AttemptBodyCapture {
    buffer: Vec<u8>,
    truncated: bool,
}

impl AttemptBodyCapture {
    pub(crate) fn append(&mut self, bytes: &[u8]) {
        if bytes.is_empty() || self.truncated {
            return;
        }
        let max_bytes = crate::execution_runtime::MAX_STREAM_BODY_CAPTURE_BYTES;
        if self.buffer.len() >= max_bytes {
            self.truncated = true;
            return;
        }
        let remaining = max_bytes - self.buffer.len();
        let copied = bytes.len().min(remaining);
        self.buffer.extend_from_slice(&bytes[..copied]);
        if copied < bytes.len() {
            self.truncated = true;
        }
    }

    pub(crate) fn encode(&self) -> (Option<String>, Option<UsageBodyCaptureState>) {
        self.encode_with_limit(crate::execution_runtime::MAX_STREAM_BODY_CAPTURE_BYTES)
    }

    fn encode_with_limit(
        &self,
        max_bytes: usize,
    ) -> (Option<String>, Option<UsageBodyCaptureState>) {
        let captured = &self.buffer[..self.buffer.len().min(max_bytes)];
        let body = (!captured.is_empty())
            .then(|| base64::engine::general_purpose::STANDARD.encode(captured));
        let state = if self.truncated || captured.len() < self.buffer.len() {
            UsageBodyCaptureState::Truncated
        } else if captured.is_empty() {
            UsageBodyCaptureState::None
        } else {
            UsageBodyCaptureState::Inline
        };
        (body, Some(state))
    }
}

/// 启动一次 attempt 记账所需的种子。
pub(crate) struct AttemptLifecycleSeed {
    pub(crate) plan: ExecutionPlan,
    pub(crate) report_kind: String,
    pub(crate) report_context: Option<Value>,
    pub(crate) stage_guard: AttemptStageGuard,
}

/// 结算一次 attempt 需要的终态事实，由 transport 提供。
pub(crate) struct AttemptTerminalFactsInput<'a> {
    pub(crate) facts: AttemptTerminalFacts,
    pub(crate) terminal_summary: ExecutionStreamTerminalSummary,
    pub(crate) telemetry: ExecutionTelemetry,
    pub(crate) provider_headers: BTreeMap<String, String>,
    pub(crate) provider_body: &'a AttemptBodyCapture,
    pub(crate) client_body: &'a AttemptBodyCapture,
    /// 供应商终态载荷原文（`error` / `response.failed` 一类），失败效果用它做
    /// failover 分类，优先级高于摘要里的 parser_error。
    pub(crate) provider_error_body: Option<&'a str>,
    /// 人类可读的结算原因，写进 candidate 行。
    pub(crate) reason: &'a str,
}

/// 一次 provider attempt 的记账生命周期：pending → started → terminal。
pub(crate) struct ExecutionAttemptLifecycle {
    plan: ExecutionPlan,
    trace_id: String,
    report_kind: String,
    report_context: Option<Value>,
    candidate_started_at_unix_ms: u64,
    stage_guard: AttemptStageGuard,
    started_recorded: bool,
}

impl ExecutionAttemptLifecycle {
    /// 写 Pending usage 行 + Pending candidate slot。
    ///
    /// 两个写入都不设上界：此时还没有任何东西可以兜底，行没写成功就等于这次
    /// attempt 不存在。
    pub(crate) async fn begin(state: &AppState, seed: AttemptLifecycleSeed) -> Self {
        let AttemptLifecycleSeed {
            plan,
            report_kind,
            report_context,
            stage_guard,
        } = seed;

        let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
        // Keep every transport on the same lifecycle data path. `AppState` can
        // dedicate an isolated background database pool to usage writes; using
        // the foreground state here bypasses that path and leaves the caller
        // with a different persistence lifecycle.
        let usage_data = state.usage_lifecycle_data_state().as_ref().clone();
        state
            .usage_runtime
            .record_pending_direct(&usage_data, lifecycle_seed)
            .await;

        let candidate_started_at_unix_ms = current_unix_ms();
        record_local_request_candidate_status(
            state,
            &plan,
            report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Pending,
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                started_at_unix_ms: Some(candidate_started_at_unix_ms),
                finished_at_unix_ms: None,
            },
        )
        .await;

        Self {
            trace_id: plan.request_id.clone(),
            plan,
            report_kind,
            report_context,
            candidate_started_at_unix_ms,
            stage_guard,
            started_recorded: false,
        }
    }

    pub(crate) fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    pub(crate) fn trace_id(&self) -> &str {
        self.trace_id.as_str()
    }

    pub(crate) fn report_context(&self) -> Option<&Value> {
        self.report_context.as_ref()
    }

    pub(crate) fn take_report_context(&mut self) -> Option<Value> {
        self.report_context.take()
    }

    pub(crate) fn set_report_context(&mut self, report_context: Option<Value>) {
        self.report_context = report_context;
    }

    pub(crate) const fn stage_guard(&self) -> AttemptStageGuard {
        self.stage_guard
    }

    /// 首个可计费事件到达：usage stream_started + candidate Streaming。幂等。
    pub(crate) async fn mark_started(
        &mut self,
        state: &AppState,
        status_code: u16,
        telemetry: &ExecutionTelemetry,
    ) {
        if self.started_recorded {
            return;
        }
        self.started_recorded = true;
        let lifecycle_seed = build_lifecycle_usage_seed(&self.plan, self.report_context.as_ref());
        state.usage_runtime.record_stream_started(
            state.usage_lifecycle_data_state().as_ref(),
            &lifecycle_seed,
            status_code,
            Some(telemetry),
        );
        let _ = self
            .stage_guard
            .await_stage(
                self.trace_id.as_str(),
                "candidate_stream_started",
                record_local_request_candidate_status(
                    state,
                    &self.plan,
                    self.report_context.as_ref(),
                    SchedulerRequestCandidateStatusUpdate {
                        status: RequestCandidateStatus::Streaming,
                        status_code: Some(status_code),
                        error_type: None,
                        error_message: None,
                        latency_ms: None,
                        started_at_unix_ms: Some(self.candidate_started_at_unix_ms),
                        finished_at_unix_ms: None,
                    },
                ),
            )
            .await;
    }

    /// 终态四段，顺序不可重排：
    ///
    /// 1. usage terminal —— 这次 attempt 的账单记录。行是以 Pending 建立的，
    ///    没有别的东西会去对账，所以用 detachable 保证不丢。
    /// 2. candidate terminal —— 调度侧的终态，慢依赖不能让它停在 Streaming。
    /// 3. provider 效果 —— health / adaptive / pool 反馈，次于前两段；超时后
    ///    仍然兜底释放 pool key lease，否则要等 lease TTL 过期才放出这把 key。
    /// 4. execution report —— 作废账单的分支不提交（与 HTTP 在下游断开后
    ///    同样不提交一致）。
    pub(crate) async fn settle(
        mut self,
        state: &AppState,
        input: AttemptTerminalFactsInput<'_>,
    ) -> AttemptSettlement {
        let AttemptTerminalFactsInput {
            facts,
            terminal_summary,
            telemetry,
            provider_headers,
            provider_body,
            client_body,
            provider_error_body,
            reason,
        } = input;

        let (provider_body_base64, provider_body_state) = provider_body.encode();
        let (client_body_base64, client_body_state) = client_body.encode();
        let payload = GatewayStreamReportRequest {
            trace_id: self.trace_id.clone(),
            report_kind: std::mem::take(&mut self.report_kind),
            report_context: self.report_context.take(),
            status_code: attempt_status_code(facts),
            headers: provider_headers,
            provider_body_base64,
            provider_body_state,
            client_body_base64,
            client_body_state,
            terminal_summary: Some(terminal_summary.clone()),
            telemetry: Some(telemetry),
        };
        let settlement = classify_attempt_settlement(AttemptSettlementInputs {
            facts,
            report_represents_failure: stream_report_represents_failure(&payload),
            observed_finish: terminal_summary.observed_finish,
            has_parser_error: terminal_summary.parser_error.is_some(),
        });

        // 1. usage terminal
        let context_seed =
            build_terminal_usage_context_seed(&self.plan, payload.report_context.as_ref());
        let payload_seed = build_stream_terminal_usage_payload_seed(&payload);
        let billing_void = settlement.billing.is_void();
        let usage_runtime = Arc::clone(&state.usage_runtime);
        let usage_data = Arc::clone(state.usage_lifecycle_data_state());
        let usage_producer = usage_runtime.track_producer();
        self.stage_guard
            .await_detachable_stage(self.trace_id.as_str(), "usage_terminal", async move {
                let _usage_producer = usage_producer;
                usage_runtime
                    .record_stream_terminal(
                        usage_data.as_ref(),
                        context_seed,
                        payload_seed,
                        billing_void,
                    )
                    .await;
            })
            .await;

        // 2. candidate terminal
        let (error_type, error_message) = candidate_error_fields(
            settlement.candidate_error,
            terminal_summary.parser_error.as_deref(),
            reason,
        );
        let candidate_state = state.clone();
        let candidate_plan = self.plan.clone();
        let candidate_report_context = payload.report_context.clone();
        let candidate_update = SchedulerRequestCandidateStatusUpdate {
            status: match settlement.candidate_status {
                AttemptCandidateStatus::Cancelled => RequestCandidateStatus::Cancelled,
                AttemptCandidateStatus::Failed => RequestCandidateStatus::Failed,
                AttemptCandidateStatus::Success => RequestCandidateStatus::Success,
            },
            status_code: Some(settlement.status_code),
            error_type,
            error_message,
            latency_ms: payload
                .telemetry
                .as_ref()
                .and_then(|value| value.elapsed_ms),
            started_at_unix_ms: Some(self.candidate_started_at_unix_ms),
            finished_at_unix_ms: Some(current_unix_ms()),
        };
        self.stage_guard
            .await_detachable_stage(self.trace_id.as_str(), "candidate_terminal", async move {
                record_local_request_candidate_status(
                    &candidate_state,
                    &candidate_plan,
                    candidate_report_context.as_ref(),
                    candidate_update,
                )
                .await;
            })
            .await;

        // 3. provider 效果
        let effect_context = LocalExecutionEffectContext {
            plan: &self.plan,
            report_context: payload.report_context.as_ref(),
        };
        let effects_completed = self
            .stage_guard
            .await_stage(self.trace_id.as_str(), "provider_effects", async {
                match settlement.provider_effect {
                    AttemptProviderEffect::ReleasePoolKeyLease => {
                        release_local_pool_key_lease(state, effect_context).await;
                    }
                    AttemptProviderEffect::ProviderFailure => {
                        let response_text = provider_error_body
                            .or(terminal_summary.parser_error.as_deref())
                            .unwrap_or(reason);
                        let mut effect = LocalStreamFailureEffect::new(
                            settlement.status_code,
                            &payload.headers,
                            Some(response_text),
                        );
                        if facts.provider.stream_timeout() {
                            effect = effect.with_stream_timeout();
                        }
                        apply_local_stream_failure_effects(state, effect_context, effect).await;
                    }
                    AttemptProviderEffect::ProviderSuccess => {
                        apply_local_stream_success_effects(state, effect_context, &payload).await;
                    }
                }
            })
            .await
            .is_some();
        if !effects_completed {
            // The provider-effects future was dropped at the caller's stage bound.  Lease
            // cleanup must not be dropped by that same bound as well: retain an owned copy of
            // the exact report context (including its lease token/fencing token) and let the
            // cleanup task finish after the caller stops waiting.  The underlying conditional
            // release is idempotent, and a context without a lease is a no-op.
            let release_state = state.clone();
            let release_plan = self.plan.clone();
            let release_report_context = payload.report_context.clone();
            self.stage_guard
                .await_detachable_stage(
                    self.trace_id.as_str(),
                    "pool_lease_release_after_effect_timeout",
                    async move {
                        release_local_pool_key_lease(
                            &release_state,
                            LocalExecutionEffectContext {
                                plan: &release_plan,
                                report_context: release_report_context.as_ref(),
                            },
                        )
                        .await;
                    },
                )
                .await;
        }

        // 4. execution report
        if settlement.submit_execution_report {
            if let Some(Err(error)) = self
                .stage_guard
                .await_stage(
                    self.trace_id.as_str(),
                    "execution_report",
                    submit_stream_report(state, payload),
                )
                .await
            {
                warn!(
                    event_name = "execution_attempt_report_submit_failed",
                    log_type = "ops",
                    trace_id = %self.trace_id,
                    error = ?error,
                    "gateway failed to submit an execution attempt terminal report"
                );
            }
        }

        settlement
    }
}

/// candidate 行上的 error_type / error_message。
fn candidate_error_fields(
    candidate_error: AttemptCandidateError,
    parser_error: Option<&str>,
    reason: &str,
) -> (Option<String>, Option<String>) {
    match candidate_error {
        AttemptCandidateError::Cancelled => (
            Some("websocket_cancelled".to_string()),
            Some(reason.to_string()),
        ),
        AttemptCandidateError::ClientDeliveryFailed => (
            Some("client_delivery_failed".to_string()),
            Some(reason.to_string()),
        ),
        AttemptCandidateError::MissingTerminal => (
            Some("stream_missing_terminal_event".to_string()),
            Some(parser_error.map(str::to_string).unwrap_or_else(|| {
                "upstream Responses WebSocket ended before a provider terminal event".to_string()
            })),
        ),
        AttemptCandidateError::TerminalError => (
            Some("stream_terminal_error".to_string()),
            parser_error
                .map(str::to_string)
                .or_else(|| Some(reason.to_string())),
        ),
        AttemptCandidateError::None => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_attempt_provider_effect, classify_attempt_settlement, AttemptBilling,
        AttemptCandidateError, AttemptCandidateStatus, AttemptClientDelivery,
        AttemptProviderEffect, AttemptProviderOutcome, AttemptSettlement, AttemptSettlementInputs,
        AttemptTerminalFacts,
    };

    fn settle(
        provider: AttemptProviderOutcome,
        delivery: AttemptClientDelivery,
        report_represents_failure: bool,
        observed_finish: bool,
        has_parser_error: bool,
    ) -> AttemptSettlement {
        classify_attempt_settlement(AttemptSettlementInputs {
            facts: AttemptTerminalFacts { provider, delivery },
            report_represents_failure,
            observed_finish,
            has_parser_error,
        })
    }

    const fn terminal(status_code: u16) -> AttemptProviderOutcome {
        AttemptProviderOutcome::Terminal {
            status_code,
            cancelled_by_provider: false,
        }
    }

    const fn provider_cancelled() -> AttemptProviderOutcome {
        AttemptProviderOutcome::Terminal {
            status_code: 499,
            cancelled_by_provider: true,
        }
    }

    const fn aborted(status_code: u16, reason: &'static str) -> AttemptProviderOutcome {
        AttemptProviderOutcome::Aborted {
            status_code,
            reason,
            stream_timeout: status_code == 504,
        }
    }

    /// 投递失败时 `forced_error` 必须为 `None`：客户端走了不是供应商的错误。
    /// 与现状 `ResponsesWebSocketTurnOutcome::forced_error()` 对 `Cancelled`
    /// 返回 `None` 一致。
    #[test]
    fn only_a_provider_abort_with_complete_delivery_is_a_forced_error() {
        assert_eq!(
            AttemptTerminalFacts {
                provider: aborted(502, "upstream failed"),
                delivery: AttemptClientDelivery::Complete,
            }
            .forced_error(),
            Some("upstream failed")
        );
        assert_eq!(
            AttemptTerminalFacts {
                provider: aborted(499, "client went away"),
                delivery: AttemptClientDelivery::Aborted {
                    reason: "client went away"
                },
            }
            .forced_error(),
            None
        );
        assert_eq!(
            AttemptTerminalFacts {
                provider: terminal(200),
                delivery: AttemptClientDelivery::Complete,
            }
            .forced_error(),
            None
        );
    }

    #[test]
    fn the_recorded_reason_prefers_the_client_delivery_failure() {
        assert_eq!(
            AttemptTerminalFacts {
                provider: terminal(200),
                delivery: AttemptClientDelivery::Aborted {
                    reason: "client went away"
                },
            }
            .reason(),
            "client went away"
        );
        assert_eq!(
            AttemptTerminalFacts {
                provider: provider_cancelled(),
                delivery: AttemptClientDelivery::Complete,
            }
            .reason(),
            "provider cancelled the response"
        );
        assert_eq!(
            AttemptTerminalFacts {
                provider: terminal(200),
                delivery: AttemptClientDelivery::Complete,
            }
            .reason(),
            "provider returned a terminal response event"
        );
        assert_eq!(
            AttemptTerminalFacts {
                provider: aborted(502, "upstream failed"),
                delivery: AttemptClientDelivery::Complete,
            }
            .reason(),
            "upstream failed"
        );
    }

    /// §1.6 结算表，逐行。
    #[test]
    fn settlement_table_row_provider_cancelled_is_void_regardless_of_delivery() {
        for delivery in [
            AttemptClientDelivery::Complete,
            AttemptClientDelivery::Aborted { reason: "gone" },
        ] {
            for report_represents_failure in [false, true] {
                let settlement = settle(
                    provider_cancelled(),
                    delivery,
                    report_represents_failure,
                    true,
                    false,
                );
                assert_eq!(
                    settlement,
                    AttemptSettlement {
                        status_code: 499,
                        billing: AttemptBilling::Void,
                        candidate_status: AttemptCandidateStatus::Cancelled,
                        candidate_error: AttemptCandidateError::Cancelled,
                        provider_effect: AttemptProviderEffect::ReleasePoolKeyLease,
                        submit_execution_report: false,
                    },
                    "delivery={delivery:?} report_failure={report_represents_failure}"
                );
            }
        }
    }

    #[test]
    fn settlement_table_row_aborted_provider_with_aborted_delivery_is_void() {
        let settlement = settle(
            aborted(499, "client went away"),
            AttemptClientDelivery::Aborted {
                reason: "client went away",
            },
            true,
            false,
            false,
        );
        assert_eq!(
            settlement,
            AttemptSettlement {
                status_code: 499,
                billing: AttemptBilling::Void,
                candidate_status: AttemptCandidateStatus::Cancelled,
                candidate_error: AttemptCandidateError::Cancelled,
                provider_effect: AttemptProviderEffect::ReleasePoolKeyLease,
                submit_execution_report: false,
            }
        );
    }

    #[test]
    fn settlement_table_row_clean_provider_terminal_is_a_billed_success() {
        let settlement = settle(
            terminal(200),
            AttemptClientDelivery::Complete,
            false,
            true,
            false,
        );
        assert_eq!(
            settlement,
            AttemptSettlement {
                status_code: 200,
                billing: AttemptBilling::Billed,
                candidate_status: AttemptCandidateStatus::Success,
                candidate_error: AttemptCandidateError::None,
                provider_effect: AttemptProviderEffect::ProviderSuccess,
                submit_execution_report: true,
            }
        );
    }

    /// 合法 `response.incomplete`：记账层判失败，但供应商工作正常，
    /// 不扣健康分、只释放 lease，并且账单照记。
    #[test]
    fn settlement_table_row_legitimate_incomplete_is_billed_without_provider_failure() {
        let settlement = settle(
            terminal(200),
            AttemptClientDelivery::Complete,
            true,
            true,
            false,
        );
        assert_eq!(
            settlement,
            AttemptSettlement {
                status_code: 200,
                billing: AttemptBilling::Billed,
                candidate_status: AttemptCandidateStatus::Failed,
                candidate_error: AttemptCandidateError::TerminalError,
                provider_effect: AttemptProviderEffect::ReleasePoolKeyLease,
                submit_execution_report: true,
            }
        );
    }

    #[test]
    fn settlement_table_row_provider_abort_projects_a_provider_failure() {
        let settlement = settle(
            aborted(
                502,
                "upstream WebSocket closed before provider terminal event",
            ),
            AttemptClientDelivery::Complete,
            true,
            false,
            false,
        );
        assert_eq!(
            settlement,
            AttemptSettlement {
                status_code: 502,
                billing: AttemptBilling::Billed,
                candidate_status: AttemptCandidateStatus::Failed,
                candidate_error: AttemptCandidateError::MissingTerminal,
                provider_effect: AttemptProviderEffect::ProviderFailure,
                submit_execution_report: true,
            }
        );
    }

    /// ✱ 修正后的那一行：provider 终态已到达，客户端投递失败不再作废账单。
    ///
    /// 供应商已经完成推理并消耗 token，客户端还能用 `previous_response_id`
    /// 续取这条响应；把成本记成 0 等于让上游账单凭空消失。投递失败作为独立
    /// 事实留在 candidate 的错误分类里。
    #[test]
    fn settlement_table_row_client_delivery_failure_keeps_a_reached_terminal_billed() {
        let settlement = settle(
            terminal(200),
            AttemptClientDelivery::Aborted {
                reason: "gateway could not relay the provider event to the client",
            },
            false,
            true,
            false,
        );
        assert_eq!(
            settlement,
            AttemptSettlement {
                status_code: 200,
                billing: AttemptBilling::Billed,
                candidate_status: AttemptCandidateStatus::Success,
                candidate_error: AttemptCandidateError::ClientDeliveryFailed,
                provider_effect: AttemptProviderEffect::ProviderSuccess,
                submit_execution_report: true,
            }
        );

        // 除了 candidate 的错误分类，其余判定与「投递成功」完全一致。
        let delivered = settle(
            terminal(200),
            AttemptClientDelivery::Complete,
            false,
            true,
            false,
        );
        assert_eq!(settlement.status_code, delivered.status_code);
        assert_eq!(settlement.billing, delivered.billing);
        assert_eq!(settlement.candidate_status, delivered.candidate_status);
        assert_eq!(settlement.provider_effect, delivered.provider_effect);
        assert_eq!(
            settlement.submit_execution_report,
            delivered.submit_execution_report
        );
        assert_ne!(settlement.candidate_error, delivered.candidate_error);
    }

    /// 供应商还没给出终态时，客户端投递失败仍然作废账单：这一轮确实没有产出。
    #[test]
    fn a_delivery_failure_without_a_provider_terminal_still_voids_the_bill() {
        let settlement = settle(
            aborted(499, "client went away"),
            AttemptClientDelivery::Aborted {
                reason: "client went away",
            },
            false,
            false,
            false,
        );
        assert_eq!(settlement.status_code, 499);
        assert_eq!(settlement.billing, AttemptBilling::Void);
        assert_eq!(
            settlement.candidate_status,
            AttemptCandidateStatus::Cancelled
        );
        assert_eq!(settlement.candidate_error, AttemptCandidateError::Cancelled);
        assert!(!settlement.submit_execution_report);
    }

    /// 供应商自己声明取消时，即使内容送到了客户端也不计费。
    #[test]
    fn a_provider_declared_cancellation_is_void_even_when_delivered() {
        let settlement = settle(
            provider_cancelled(),
            AttemptClientDelivery::Complete,
            false,
            true,
            false,
        );
        assert_eq!(settlement.billing, AttemptBilling::Void);
        assert_eq!(settlement.candidate_error, AttemptCandidateError::Cancelled);
    }

    /// 记账层判 Success，但摘要没观察到 finish：现状会写出
    /// 「candidate=Success + error_type=stream_missing_terminal_event」，
    /// 所以状态与错误分类必须各自独立。
    #[test]
    fn a_missing_terminal_can_coexist_with_a_successful_candidate_status() {
        let settlement = settle(
            terminal(200),
            AttemptClientDelivery::Complete,
            false,
            false,
            false,
        );
        assert_eq!(settlement.candidate_status, AttemptCandidateStatus::Success);
        assert_eq!(
            settlement.candidate_error,
            AttemptCandidateError::MissingTerminal
        );
        // missing_terminal 仍然要投射供应商失败。
        assert_eq!(
            settlement.provider_effect,
            AttemptProviderEffect::ProviderFailure
        );
    }

    #[test]
    fn a_parser_error_projects_a_provider_failure_even_on_a_clean_status_code() {
        let settlement = settle(
            terminal(200),
            AttemptClientDelivery::Complete,
            true,
            true,
            true,
        );
        assert_eq!(
            settlement.provider_effect,
            AttemptProviderEffect::ProviderFailure
        );
        assert_eq!(settlement.billing, AttemptBilling::Billed);
    }

    #[test]
    fn a_legitimate_incomplete_still_releases_the_pool_key_lease() {
        // 共享 usage 判定目前仍把 response.incomplete 记成终态失败，于是会出现
        // failed=true 而 projects_provider_failure=false 的组合。这种组合必须
        // 明确落到「只释放 lease」的分支，否则 lease 会挂到 TTL 过期。
        let effect = classify_attempt_provider_effect(false, false, true);

        assert_eq!(effect, AttemptProviderEffect::ReleasePoolKeyLease);
        assert!(effect.releases_pool_key_lease());
    }

    #[test]
    fn every_provider_effect_releases_the_pool_key_lease() {
        for (cancelled, projects_provider_failure, failed, expected) in [
            (
                true,
                false,
                false,
                AttemptProviderEffect::ReleasePoolKeyLease,
            ),
            (true, true, true, AttemptProviderEffect::ReleasePoolKeyLease),
            (false, true, true, AttemptProviderEffect::ProviderFailure),
            (
                false,
                false,
                true,
                AttemptProviderEffect::ReleasePoolKeyLease,
            ),
            (false, false, false, AttemptProviderEffect::ProviderSuccess),
        ] {
            let effect =
                classify_attempt_provider_effect(cancelled, projects_provider_failure, failed);
            assert_eq!(
                effect, expected,
                "cancelled={cancelled} projects_provider_failure={projects_provider_failure} failed={failed}"
            );
            assert!(
                effect.releases_pool_key_lease(),
                "every effect branch must release the pool key lease"
            );
        }
    }

    /// 每一个结算分支都必须释放 lease：这条不变量跨越整张结算表。
    #[test]
    fn every_settlement_branch_releases_the_pool_key_lease() {
        let providers = [
            terminal(200),
            terminal(429),
            provider_cancelled(),
            aborted(502, "upstream failed"),
            aborted(504, "timed out"),
        ];
        let deliveries = [
            AttemptClientDelivery::Complete,
            AttemptClientDelivery::Aborted { reason: "gone" },
        ];
        for provider in providers {
            for delivery in deliveries {
                for report_represents_failure in [false, true] {
                    for observed_finish in [false, true] {
                        for has_parser_error in [false, true] {
                            let settlement = settle(
                                provider,
                                delivery,
                                report_represents_failure,
                                observed_finish,
                                has_parser_error,
                            );
                            assert!(
                                settlement.provider_effect.releases_pool_key_lease(),
                                "provider={provider:?} delivery={delivery:?}"
                            );
                            // 作废账单的分支一律不提交 execution report。
                            assert_eq!(
                                settlement.submit_execution_report,
                                !settlement.billing.is_void(),
                                "provider={provider:?} delivery={delivery:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod stage_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use base64::Engine as _;
    use tokio::sync::Notify;

    use super::{
        candidate_error_fields, AttemptBodyCapture, AttemptCandidateError, AttemptStageGuard,
    };

    /// 效果段超时后仍然必须释放 pool key lease，否则那把 key 要等 lease TTL
    /// 过期才放出来。调用方对兜底清理的等待也必须有界，但不能把 owned cleanup
    /// 一并取消。
    #[tokio::test]
    async fn a_timed_out_effect_stage_detaches_lease_cleanup_until_it_completes() {
        let guard = AttemptStageGuard::Bounded(Duration::from_millis(20));
        let lease_released = Arc::new(AtomicBool::new(false));
        let allow_release = Arc::new(Notify::new());
        let release_completed = Arc::new(Notify::new());

        // 第一段：永不完成的效果投射。
        let effects_completed = guard
            .await_stage("trace", "provider_effects", std::future::pending::<()>())
            .await
            .is_some();
        assert!(
            !effects_completed,
            "a stage that never completes must not report success"
        );

        // 生产代码据此走 owned/detached 兜底释放。让清理刻意慢于 caller bound，
        // 证明调用方先返回之后，清理任务仍然存活并最终完成。
        if !effects_completed {
            let released = Arc::clone(&lease_released);
            let allow_release = Arc::clone(&allow_release);
            let release_completed_task = Arc::clone(&release_completed);
            guard
                .await_detachable_stage(
                    "trace",
                    "pool_lease_release_after_effect_timeout",
                    async move {
                        allow_release.notified().await;
                        released.store(true, Ordering::SeqCst);
                        release_completed_task.notify_one();
                    },
                )
                .await;
        }
        assert!(
            !lease_released.load(Ordering::SeqCst),
            "the caller must stop waiting at its bound even while cleanup is pending"
        );

        allow_release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), release_completed.notified())
            .await
            .expect("detached lease cleanup must eventually complete");
        assert!(lease_released.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn an_unbounded_stage_guard_waits_for_the_stage() {
        let guard = AttemptStageGuard::Unbounded;
        // 无上界：即使这一段比任何 Bounded 上界都久，也必须等到它完成。
        let value = guard
            .await_stage("trace", "stage", async {
                tokio::time::sleep(Duration::from_millis(120)).await;
                7_u8
            })
            .await;
        assert_eq!(value, Some(7));
    }

    /// candidate 终态不能因为调用方的等待上界而丢失：先 spawn 再等，超时只
    /// 停止 relay 对它的等待，后台写入仍然必须完成。
    #[tokio::test]
    async fn a_detachable_candidate_terminal_completes_after_the_caller_stops_waiting() {
        let guard = AttemptStageGuard::Bounded(Duration::from_millis(20));
        let written = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&written);

        guard
            .await_detachable_stage("trace", "candidate_terminal", async move {
                tokio::time::sleep(Duration::from_millis(120)).await;
                flag.store(true, Ordering::SeqCst);
            })
            .await;
        assert!(
            !written.load(Ordering::SeqCst),
            "the caller must stop waiting at its bound"
        );

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            written.load(Ordering::SeqCst),
            "a detached candidate terminal write must still run to completion"
        );
    }

    /// settle 的四段顺序不可重排：账单先落地，然后 candidate 终态，然后次要的
    /// 供应商效果，最后才是 execution report。用计数器替身记录实际顺序。
    #[tokio::test]
    async fn the_settle_stages_run_in_a_fixed_order() {
        let guard = AttemptStageGuard::Bounded(Duration::from_millis(500));
        let order = Arc::new(Mutex::new(Vec::new()));

        for stage in [
            "usage_terminal",
            "candidate_terminal",
            "provider_effects",
            "execution_report",
        ] {
            let recorder = Arc::clone(&order);
            let _ = guard
                .await_stage("trace", stage, async move {
                    recorder.lock().expect("order lock").push(stage);
                })
                .await;
        }

        assert_eq!(
            order.lock().expect("order lock").as_slice(),
            [
                "usage_terminal",
                "candidate_terminal",
                "provider_effects",
                "execution_report",
            ]
        );
    }

    /// body capture 的编码状态。Full 记录级别仍受 gateway 的硬上限约束，
    /// 这样长连接不会把审计副本无限累积。
    #[test]
    fn body_capture_encodes_inline_and_empty_states() {
        assert_eq!(
            crate::execution_runtime::MAX_STREAM_BODY_CAPTURE_BYTES,
            crate::execution_runtime::MAX_STREAM_BODY_CAPTURE_BYTES
        );

        let mut capture = AttemptBodyCapture::default();
        capture.append(b"data: {\"type\":\"response.created\"}\n\n");
        capture.append(b"data: {\"type\":\"response.completed\"}\n\n");
        let (body, state) = capture.encode();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body.expect("a non-empty capture is encoded"))
            .expect("capture is valid base64");
        let decoded = String::from_utf8(decoded).expect("capture is UTF-8");
        assert!(
            decoded.starts_with("data: ") && decoded.ends_with("\n\n"),
            "the capture must stay SSE-shaped for the usage runtime: {decoded:?}"
        );
        assert_eq!(decoded.matches("data: ").count(), 2, "appends concatenate");
        assert_eq!(
            state,
            Some(aether_data_contracts::repository::usage::UsageBodyCaptureState::Inline)
        );

        let empty = AttemptBodyCapture::default();
        let (body, state) = empty.encode();
        assert!(body.is_none());
        assert_eq!(
            state,
            Some(aether_data_contracts::repository::usage::UsageBodyCaptureState::None)
        );

        let defensive = AttemptBodyCapture {
            buffer: b"abcdef".to_vec(),
            truncated: false,
        };
        let (body, state) = defensive.encode_with_limit(3);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body.expect("bounded capture should be encoded"))
            .expect("capture is valid base64");
        assert_eq!(decoded, b"abc");
        assert_eq!(
            state,
            Some(aether_data_contracts::repository::usage::UsageBodyCaptureState::Truncated)
        );
    }

    /// candidate 行的 error_type 映射：投递失败与供应商侧失败必须各有名字。
    #[test]
    fn candidate_error_fields_name_each_failure_kind() {
        assert_eq!(
            candidate_error_fields(AttemptCandidateError::None, None, "reason"),
            (None, None)
        );
        assert_eq!(
            candidate_error_fields(AttemptCandidateError::Cancelled, None, "gone").0,
            Some("websocket_cancelled".to_string())
        );
        assert_eq!(
            candidate_error_fields(
                AttemptCandidateError::ClientDeliveryFailed,
                None,
                "write failed"
            ),
            (
                Some("client_delivery_failed".to_string()),
                Some("write failed".to_string())
            )
        );
        assert_eq!(
            candidate_error_fields(
                AttemptCandidateError::TerminalError,
                Some("parser"),
                "reason"
            ),
            (
                Some("stream_terminal_error".to_string()),
                Some("parser".to_string())
            )
        );
        assert_eq!(
            candidate_error_fields(AttemptCandidateError::MissingTerminal, None, "reason").0,
            Some("stream_missing_terminal_event".to_string())
        );
    }
}
