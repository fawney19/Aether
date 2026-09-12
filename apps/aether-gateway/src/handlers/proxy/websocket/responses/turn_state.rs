//! 一条 Responses WebSocket 连接上的 turn 状态机。
//!
//! 现状把「有没有正在进行的 turn」拆成 `response_in_flight`、`active_turn`、
//! `active_response_create` 三个可独立变化的字段，8 种组合里只有 3 种合法，
//! 非法组合只能靠调用点的 if 和「记得同时改另外两个字段」来避免。这里把它收敛成
//! 一个枚举：合法组合由类型保证，转换只能走受控 API。

use aether_provider_transport::CodexFingerprintConvergenceContext;
use serde_json::Value;

use super::control::ResponsesWebSocketTurnControl;
use super::lifecycle::ActiveProviderAttempt;
use super::request::response_create_has_previous_response_id;
use crate::plan_usage_policy::PlanUsagePolicySnapshot;

/// 客户端一次 `response.create` 对应的 logical turn。
///
/// 一个 logical turn 可能经历多个 provider attempt：配额透明重试会换一把 key、
/// 换一条上游连接重放同一份客户端事件，但对客户端始终是同一轮请求。
/// `client_event` 保存的必须是**已脱敏**的事件（见 `super::redaction`），
/// 因为透明重试直接重放它。
#[derive(Debug)]
pub(super) struct LogicalTurn {
    pub(super) client_event: Value,
    /// Effective `store` after provider body rules and WebSocket framing. Only
    /// an explicit provider-side `true` permits cross-connection registry
    /// state; false or absent remains ZDR/connection-local.
    pub(super) provider_store: bool,
    pub(super) turn_index: u64,
    pub(super) logical_turn_id: String,
    /// Immutable Codex client identity for every provider attempt belonging to
    /// this logical turn. A transparent re-plan must never mint a new turn.
    pub(super) codex_fingerprint_context: Option<CodexFingerprintConvergenceContext>,
    pub(super) turn_attempt: u32,
    pub(super) retry_attempted: bool,
    pub(super) retry_unsafe_reason: Option<&'static str>,
    /// Exact live control decision and strong auth snapshot used to authorize
    /// this logical turn. Quota retries reuse it instead of falling back to the
    /// connection's Upgrade-time authorization snapshot.
    pub(super) turn_control: Option<ResponsesWebSocketTurnControl>,
    /// Subscription-plan concurrency is per logical `response.create`, not per
    /// provider attempt. Keeping the permit here makes transparent retries
    /// retain the same slot through `Replanning` until the logical terminal.
    pub(super) plan_usage_permit: Option<aether_runtime::AdmissionPermit>,
    /// Cost limits are evaluated against the policy and entitlement window
    /// that admitted this logical response.create. Every transparent provider
    /// attempt reuses this snapshot while receiving its own reservation token.
    pub(super) plan_usage_policy_snapshot: Option<PlanUsagePolicySnapshot>,
}

impl LogicalTurn {
    pub(super) fn new(client_event: Value, turn_index: u64, logical_turn_id: String) -> Self {
        Self {
            client_event,
            provider_store: false,
            turn_index,
            logical_turn_id,
            codex_fingerprint_context: None,
            turn_attempt: 1,
            retry_attempted: false,
            retry_unsafe_reason: None,
            turn_control: None,
            plan_usage_permit: None,
            plan_usage_policy_snapshot: None,
        }
    }

    pub(super) fn with_turn_control(mut self, turn_control: ResponsesWebSocketTurnControl) -> Self {
        self.turn_control = Some(turn_control);
        self
    }

    pub(super) fn with_plan_usage_permit(
        mut self,
        permit: Option<aether_runtime::AdmissionPermit>,
    ) -> Self {
        self.plan_usage_permit = permit;
        self
    }

    pub(super) fn with_plan_usage_policy_snapshot(
        mut self,
        snapshot: Option<PlanUsagePolicySnapshot>,
    ) -> Self {
        self.plan_usage_policy_snapshot = snapshot;
        self
    }

    pub(super) fn with_codex_fingerprint_context(
        mut self,
        context: CodexFingerprintConvergenceContext,
    ) -> Self {
        self.codex_fingerprint_context = Some(context);
        self
    }

    pub(super) fn with_provider_store(mut self, provider_store: bool) -> Self {
        self.provider_store = provider_store;
        self
    }

    pub(super) fn quota_retry_block_reason(&self) -> Option<&'static str> {
        if self.retry_attempted {
            Some("quota_retry_already_attempted")
        } else if let Some(reason) = self.retry_unsafe_reason {
            Some(reason)
        } else if response_create_has_previous_response_id(&self.client_event) {
            Some("previous_response_id")
        } else {
            None
        }
    }

    pub(super) fn mark_retry_unsafe(&mut self, reason: &'static str) {
        self.retry_unsafe_reason.get_or_insert(reason);
    }
}

/// 连接上「有没有正在进行的 logical turn」这一唯一事实。
///
/// 类型参数只为测试留出注入点：生产代码一律用默认的
/// [`ActiveProviderAttempt`]，测试用轻量替身驱动同一套转换逻辑，
/// 不必构造 `AppState` 和真实 socket。
pub(super) enum ResponsesTurnState<A = ActiveProviderAttempt> {
    /// 没有进行中的 logical turn。上游可能仍绑定，也可能已被 detach。
    Idle,
    /// 一个 logical turn 正在等待 provider 终态：logical 与当前 attempt 同时存在。
    Responding { logical: LogicalTurn, attempt: A },
    /// logical turn 仍在，但当前 attempt 已被取走去结算或重绑，新 attempt 未就位。
    /// 配额透明重试期间就处于这个状态。
    Replanning { logical: LogicalTurn },
}

impl<A> ResponsesTurnState<A> {
    /// 上游是否有一个正在进行的 response。取代原来的 `response_in_flight` 字段。
    pub(super) const fn response_in_flight(&self) -> bool {
        matches!(self, Self::Responding { .. })
    }

    /// 是否可以接受一条新的客户端 `response.create`。
    ///
    /// `Replanning` 也要拒绝：那时旧 attempt 的结算/重绑还没收尾。实际上
    /// 透明重试整段都在 relay loop 的上游分支里同步完成，此时不会读客户端帧，
    /// 所以这条相对原来的 `response_in_flight` 判断没有行为差异。
    pub(super) const fn accepts_new_response_create(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub(super) const fn logical(&self) -> Option<&LogicalTurn> {
        match self {
            Self::Idle => None,
            Self::Responding { logical, .. } | Self::Replanning { logical } => Some(logical),
        }
    }

    pub(super) fn logical_mut(&mut self) -> Option<&mut LogicalTurn> {
        match self {
            Self::Idle => None,
            Self::Responding { logical, .. } | Self::Replanning { logical } => Some(logical),
        }
    }

    pub(super) const fn attempt(&self) -> Option<&A> {
        match self {
            Self::Responding { attempt, .. } => Some(attempt),
            Self::Idle | Self::Replanning { .. } => None,
        }
    }

    pub(super) fn attempt_mut(&mut self) -> Option<&mut A> {
        match self {
            Self::Responding { attempt, .. } => Some(attempt),
            Self::Idle | Self::Replanning { .. } => None,
        }
    }

    /// `Idle` → `Responding`：装上一个新 logical turn 及其首个 attempt。
    ///
    /// 与原来的 `active_turn = Some(..)` 语义一致：若此刻竟持有旧 attempt，
    /// 它会被丢弃并由自身的 drop guard 兜底结算，而不是静默泄漏。
    pub(super) fn begin(&mut self, logical: LogicalTurn, attempt: A) {
        debug_assert!(
            self.accepts_new_response_create(),
            "a new logical turn must only begin on an idle connection"
        );
        *self = Self::Responding { logical, attempt };
    }

    /// `Responding` → `Replanning`：把当前 attempt 交给调用方结算，保留 logical turn。
    pub(super) fn detach_attempt(&mut self) -> Option<A> {
        match std::mem::replace(self, Self::Idle) {
            Self::Responding { logical, attempt } => {
                *self = Self::Replanning { logical };
                Some(attempt)
            }
            state @ (Self::Idle | Self::Replanning { .. }) => {
                *self = state;
                None
            }
        }
    }

    /// `Replanning` → `Responding`：同一 logical turn 的下一个 attempt 就位。
    ///
    /// 状态不符时把 attempt 交还调用方，避免静默丢弃一条已经写了 pending usage
    /// 行、占着 candidate 和 pool key lease 的 attempt。
    pub(super) fn resume(&mut self, attempt: A) -> Result<(), A> {
        match std::mem::replace(self, Self::Idle) {
            Self::Replanning { logical } => {
                *self = Self::Responding { logical, attempt };
                Ok(())
            }
            state => {
                *self = state;
                Err(attempt)
            }
        }
    }

    /// `Responding`/`Replanning` → `Idle`：logical turn 结束，交出待结算的 attempt。
    ///
    /// 取代原来「`active_turn.take()` + 在每个出口手写 `active_response_create = None`」
    /// 的组合：清理只有这一个出口，漏清不再可能。
    pub(super) fn end(&mut self) -> Option<A> {
        match std::mem::replace(self, Self::Idle) {
            Self::Responding { attempt, .. } => Some(attempt),
            Self::Idle | Self::Replanning { .. } => None,
        }
    }
}

impl ResponsesTurnState {
    /// 记录「当前 attempt 的内容没能完整交付给客户端」。
    ///
    /// 这条事实写在 attempt 上而不是 logical turn 上：结算是按 attempt 进行的，
    /// 而每个 attempt 的投递结果各自独立（配额透明重试时旧 attempt 可能已经把
    /// 部分事件交付出去，新 attempt 从零开始）。
    pub(super) fn record_client_delivery_aborted(&mut self, reason: &'static str) {
        if let Some(attempt) = self.attempt_mut() {
            attempt.record_client_delivery_aborted(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{LogicalTurn, ResponsesTurnState};

    /// attempt 的测试替身：只需要能被 move，不需要 AppState 或真实 socket。
    #[derive(Debug, PartialEq, Eq)]
    struct FakeAttempt(u32);

    fn logical() -> LogicalTurn {
        LogicalTurn::new(
            json!({"type": "response.create", "model": "gpt-5.6-sol"}),
            7,
            "logical-turn".to_string(),
        )
    }

    /// 透明重试失败之后：旧 attempt 已经被 detach 并结算过，logical turn 仍停在
    /// `Replanning`。此时 `end()` 不能再交出 attempt，否则同一个 attempt 会被
    /// 结算两次（两条 usage terminal、两次 pool lease 释放）。
    #[test]
    fn ending_a_replanning_turn_does_not_hand_out_a_second_attempt() {
        let mut state = ResponsesTurnState::Idle;
        state.begin(logical(), FakeAttempt(1));

        let detached = state.detach_attempt();
        assert_eq!(
            detached,
            Some(FakeAttempt(1)),
            "the attempt is settled once"
        );
        assert!(matches!(state, ResponsesTurnState::Replanning { .. }));

        // 结算已经发生，没有第二个 attempt 可交。
        assert!(
            state.end().is_none(),
            "a replanning turn must not yield a second attempt to settle"
        );
        assert!(state.accepts_new_response_create());
    }

    #[test]
    fn idle_has_no_turn_and_accepts_a_new_response_create() {
        let state = ResponsesTurnState::<FakeAttempt>::Idle;

        assert!(!state.response_in_flight());
        assert!(state.accepts_new_response_create());
        assert!(state.logical().is_none());
        assert!(state.attempt().is_none());
    }

    #[test]
    fn beginning_a_turn_makes_the_response_in_flight_and_blocks_a_second_one() {
        let mut state = ResponsesTurnState::Idle;
        state.begin(logical(), FakeAttempt(1));

        assert!(state.response_in_flight());
        assert!(!state.accepts_new_response_create());
        assert_eq!(state.logical().map(|logical| logical.turn_index), Some(7));
        assert_eq!(state.attempt(), Some(&FakeAttempt(1)));
    }

    #[test]
    fn detaching_an_attempt_keeps_the_logical_turn_but_ends_the_in_flight_response() {
        let mut state = ResponsesTurnState::Idle;
        state.begin(logical(), FakeAttempt(1));

        assert_eq!(state.detach_attempt(), Some(FakeAttempt(1)));
        assert!(!state.response_in_flight());
        assert!(!state.accepts_new_response_create());
        assert!(state.attempt().is_none());
        assert_eq!(
            state
                .logical()
                .map(|logical| logical.logical_turn_id.clone()),
            Some("logical-turn".to_string())
        );
        // 幂等：已经没有 attempt 了，再取一次不会伪造一个出来。
        assert_eq!(state.detach_attempt(), None);
    }

    #[test]
    fn resuming_replaces_the_attempt_of_the_same_logical_turn() {
        let mut state = ResponsesTurnState::Idle;
        state.begin(logical(), FakeAttempt(1));
        state
            .logical_mut()
            .expect("a responding turn always has its logical turn")
            .turn_attempt = 2;
        let _ = state.detach_attempt();

        assert_eq!(state.resume(FakeAttempt(2)), Ok(()));
        assert!(state.response_in_flight());
        assert_eq!(state.attempt(), Some(&FakeAttempt(2)));
        assert_eq!(state.logical().map(|logical| logical.turn_attempt), Some(2));
    }

    #[test]
    fn resuming_without_a_logical_turn_hands_the_attempt_back() {
        let mut state = ResponsesTurnState::Idle;

        assert_eq!(state.resume(FakeAttempt(9)), Err(FakeAttempt(9)));
        assert!(state.accepts_new_response_create());

        state.begin(logical(), FakeAttempt(1));
        assert_eq!(state.resume(FakeAttempt(9)), Err(FakeAttempt(9)));
        assert_eq!(state.attempt(), Some(&FakeAttempt(1)));
    }

    #[test]
    fn ending_a_turn_clears_both_the_logical_turn_and_the_attempt() {
        let mut state = ResponsesTurnState::Idle;
        state.begin(logical(), FakeAttempt(1));

        assert_eq!(state.end(), Some(FakeAttempt(1)));
        assert!(state.accepts_new_response_create());
        assert!(state.logical().is_none());

        // 从 Replanning 结束时没有 attempt 要交出，但 logical turn 同样必须清掉。
        state.begin(logical(), FakeAttempt(2));
        let _ = state.detach_attempt();
        assert_eq!(state.end(), None);
        assert!(state.accepts_new_response_create());
        assert!(state.logical().is_none());
    }

    #[test]
    fn quota_retry_safety_lives_on_the_logical_turn() {
        let mut state = ResponsesTurnState::Idle;
        state.begin(logical(), FakeAttempt(1));

        assert_eq!(
            state
                .logical()
                .and_then(LogicalTurn::quota_retry_block_reason),
            None
        );
        state
            .logical_mut()
            .expect("logical turn")
            .mark_retry_unsafe("standard_response_event");
        assert_eq!(
            state
                .logical()
                .and_then(LogicalTurn::quota_retry_block_reason),
            Some("standard_response_event")
        );
        // 重绑后仍是同一个 logical turn，重放安全结论不能被 attempt 轮换洗掉。
        let _ = state.detach_attempt();
        assert_eq!(state.resume(FakeAttempt(2)), Ok(()));
        assert_eq!(
            state
                .logical()
                .and_then(LogicalTurn::quota_retry_block_reason),
            Some("standard_response_event")
        );
    }
}
