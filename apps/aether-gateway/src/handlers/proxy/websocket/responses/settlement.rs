//! Responses WebSocket 的结算信号 → 记账事实映射。
//!
//! 结算判定本身是 transport 中立的，住在
//! [`crate::execution_runtime::attempt_lifecycle`]。这里只做 WS 专属的一件事：
//! 把 relay loop 的结算触发信号 [`ResponsesWebSocketTurnOutcome`] 翻译成那两个
//! 正交事实。

use super::turn::ResponsesWebSocketTurnOutcome;
use crate::execution_runtime::attempt_lifecycle::{
    AttemptClientDelivery, AttemptProviderOutcome, AttemptTerminalFacts,
    CLIENT_CANCELLED_STATUS_CODE, STREAM_TIMEOUT_STATUS_CODE,
};

/// 把「结算触发信号」+「已观察到的 provider 终态」+「已记录的投递结果」映射成
/// 两个正交事实。
///
/// `ResponsesWebSocketTurnOutcome` 描述的是 relay loop 为什么现在结算这一
/// attempt，它对 provider 的信息量并不总是完整的：
///
/// - `ProviderTerminal` / `Failure` 本身就在描述供应商这一轮的结果，是权威的。
/// - `Cancelled` 只说明「我们为客户端或连接层面的原因停下了」，不携带任何
///   provider 信息。已经观察到的 provider 终态是独立事实，不能被它覆盖——
///   这正是评审第 5 条要求分开记录的那一处。
///
/// `recorded_delivery` 是 relay loop 明确记下的投递失败（写客户端 socket 失败）。
/// 它与结算信号推出的投递结果取「只要有一侧失败就是失败」，并优先保留明确记录
/// 的原因。
pub(super) fn attempt_facts_for_outcome(
    observed_provider_terminal: Option<AttemptProviderOutcome>,
    recorded_delivery: AttemptClientDelivery,
    settling: ResponsesWebSocketTurnOutcome,
) -> AttemptTerminalFacts {
    let facts = match settling {
        ResponsesWebSocketTurnOutcome::ProviderTerminal {
            status_code,
            cancelled,
        } => AttemptTerminalFacts {
            provider: AttemptProviderOutcome::Terminal {
                status_code,
                cancelled_by_provider: cancelled,
            },
            delivery: AttemptClientDelivery::Complete,
        },
        ResponsesWebSocketTurnOutcome::Failure {
            status_code,
            reason,
        } => AttemptTerminalFacts {
            provider: AttemptProviderOutcome::Aborted {
                status_code,
                reason,
                // 现状只有 504 一族（首事件/终态超时）会投射 pool stream timeout。
                stream_timeout: status_code == STREAM_TIMEOUT_STATUS_CODE,
            },
            delivery: AttemptClientDelivery::Complete,
        },
        ResponsesWebSocketTurnOutcome::Cancelled { reason } => AttemptTerminalFacts {
            provider: observed_provider_terminal.unwrap_or(AttemptProviderOutcome::Aborted {
                status_code: CLIENT_CANCELLED_STATUS_CODE,
                reason,
                stream_timeout: false,
            }),
            delivery: AttemptClientDelivery::Aborted { reason },
        },
    };
    AttemptTerminalFacts {
        delivery: match recorded_delivery {
            AttemptClientDelivery::Aborted { .. } => recorded_delivery,
            AttemptClientDelivery::Complete => facts.delivery,
        },
        ..facts
    }
}

/// 客户端投递失败时应该用哪个结算信号。
///
/// provider 终态已经到达就用那条终态：它是权威的 provider 事实，绝不能被
/// `client_disconnected()` 覆盖掉——那正是把已完成响应记成 void billing 的原因。
/// 供应商还没给出终态时，客户端断开才是这一 attempt 的全部结论。
pub(super) fn settle_signal_for_client_delivery_failure(
    terminal_outcome: Option<ResponsesWebSocketTurnOutcome>,
) -> ResponsesWebSocketTurnOutcome {
    terminal_outcome.unwrap_or_else(ResponsesWebSocketTurnOutcome::client_disconnected)
}

#[cfg(test)]
mod tests {
    use super::super::turn::ResponsesWebSocketTurnOutcome;
    use super::{attempt_facts_for_outcome, settle_signal_for_client_delivery_failure};
    use crate::execution_runtime::attempt_lifecycle::{
        AttemptClientDelivery, AttemptProviderOutcome, AttemptTerminalFacts,
    };

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

    /// §1.6 现状 outcome → 双事实映射表，逐行。
    #[test]
    fn every_settle_signal_maps_to_a_provider_outcome_and_a_client_delivery() {
        assert_eq!(
            attempt_facts_for_outcome(
                None,
                AttemptClientDelivery::Complete,
                ResponsesWebSocketTurnOutcome::ProviderTerminal {
                    status_code: 200,
                    cancelled: false,
                },
            ),
            AttemptTerminalFacts {
                provider: terminal(200),
                delivery: AttemptClientDelivery::Complete,
            }
        );
        assert_eq!(
            attempt_facts_for_outcome(
                None,
                AttemptClientDelivery::Complete,
                ResponsesWebSocketTurnOutcome::ProviderTerminal {
                    status_code: 499,
                    cancelled: true,
                },
            ),
            AttemptTerminalFacts {
                provider: provider_cancelled(),
                delivery: AttemptClientDelivery::Complete,
            }
        );
        assert_eq!(
            attempt_facts_for_outcome(
                None,
                AttemptClientDelivery::Complete,
                ResponsesWebSocketTurnOutcome::upstream_closed()
            ),
            AttemptTerminalFacts {
                provider: aborted(
                    502,
                    "upstream WebSocket closed before provider terminal event"
                ),
                delivery: AttemptClientDelivery::Complete,
            }
        );
        assert_eq!(
            attempt_facts_for_outcome(
                None,
                AttemptClientDelivery::Complete,
                ResponsesWebSocketTurnOutcome::client_disconnected()
            ),
            AttemptTerminalFacts {
                provider: aborted(499, "client disconnected before provider terminal event"),
                delivery: AttemptClientDelivery::Aborted {
                    reason: "client disconnected before provider terminal event",
                },
            }
        );

        // 超时一族必须保留 stream_timeout 标记，否则 pool stream timeout 效果丢失。
        let first_event_timeout = attempt_facts_for_outcome(
            None,
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::first_event_timeout(),
        );
        assert!(first_event_timeout.provider.stream_timeout());
        let terminal_timeout = attempt_facts_for_outcome(
            None,
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::terminal_timeout(),
        );
        assert!(terminal_timeout.provider.stream_timeout());
        // 非 504 的失败不得被当成流式超时。
        assert!(!attempt_facts_for_outcome(
            None,
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::upstream_closed()
        )
        .provider
        .stream_timeout());
        // provider 终态即使状态码是 504 也不投射 stream timeout：现状
        // `stream_timeout()` 只匹配 Failure 分支。
        assert!(!attempt_facts_for_outcome(
            None,
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::ProviderTerminal {
                status_code: 504,
                cancelled: false,
            },
        )
        .provider
        .stream_timeout());
    }

    /// `Cancelled` 不携带 provider 信息，已观察到的终态不能被它覆盖；
    /// `ProviderTerminal` / `Failure` 本身就是权威的 provider 事实。
    #[test]
    fn an_observed_provider_terminal_survives_a_client_side_cancellation() {
        let observed = terminal(200);

        let facts = attempt_facts_for_outcome(
            Some(observed),
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::client_disconnected(),
        );
        assert_eq!(facts.provider, observed);
        assert_eq!(
            facts.delivery,
            AttemptClientDelivery::Aborted {
                reason: "client disconnected before provider terminal event",
            }
        );

        // 权威信号不被已记录事实改写。
        let facts = attempt_facts_for_outcome(
            Some(observed),
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::upstream_closed(),
        );
        assert_eq!(
            facts.provider,
            aborted(
                502,
                "upstream WebSocket closed before provider terminal event"
            )
        );
        assert_eq!(facts.delivery, AttemptClientDelivery::Complete);
    }

    /// 结算信号的选择：provider 终态已到达就用它，否则才是 client 断开。
    /// 这是修正的核心——旧实现无条件用 client_disconnected() 覆盖，
    /// 于是已完成的响应被记成 void billing。
    #[test]
    fn a_reached_terminal_is_the_settle_signal_for_a_delivery_failure() {
        let terminal_outcome = ResponsesWebSocketTurnOutcome::ProviderTerminal {
            status_code: 200,
            cancelled: false,
        };
        assert_eq!(
            settle_signal_for_client_delivery_failure(Some(terminal_outcome)),
            terminal_outcome
        );
        assert_eq!(
            settle_signal_for_client_delivery_failure(None),
            ResponsesWebSocketTurnOutcome::client_disconnected()
        );
    }

    /// 明确记录的投递失败不会被结算信号推出的「投递成功」覆盖。
    #[test]
    fn a_recorded_delivery_failure_survives_a_provider_terminal_settle_signal() {
        let facts = attempt_facts_for_outcome(
            Some(terminal(200)),
            AttemptClientDelivery::Aborted {
                reason: "write failed",
            },
            ResponsesWebSocketTurnOutcome::ProviderTerminal {
                status_code: 200,
                cancelled: false,
            },
        );
        assert_eq!(facts.provider, terminal(200));
        assert_eq!(
            facts.delivery,
            AttemptClientDelivery::Aborted {
                reason: "write failed"
            }
        );
        // 投递失败不是供应商的错误，摘要不该因此补 parser_error。
        assert_eq!(facts.forced_error(), None);
    }

    #[test]
    fn plan_permit_loss_is_gateway_cancellation_not_provider_failure() {
        let facts = attempt_facts_for_outcome(
            None,
            AttemptClientDelivery::Complete,
            ResponsesWebSocketTurnOutcome::connection_admission_lost(),
        );

        assert_eq!(
            facts.provider,
            aborted(
                499,
                "gateway WebSocket connection admission became unhealthy"
            )
        );
        assert_eq!(
            facts.delivery,
            AttemptClientDelivery::Aborted {
                reason: "gateway WebSocket connection admission became unhealthy"
            }
        );
        assert_eq!(facts.forced_error(), None);
    }
}
