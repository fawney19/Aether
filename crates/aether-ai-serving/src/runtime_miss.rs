pub trait AiRuntimeMissDiagnosticPort: Send + Sync {
    type Decision: Send + Sync;
    type Diagnostic: Send;

    fn build_runtime_miss_diagnostic(
        &self,
        decision: &Self::Decision,
        plan_kind: &str,
        requested_model: Option<&str>,
        reason: &str,
    ) -> Self::Diagnostic;

    fn set_candidate_count(&self, diagnostic: &mut Self::Diagnostic, candidate_count: usize);

    fn apply_candidate_evaluation_progress(
        &self,
        diagnostic: &mut Self::Diagnostic,
        candidate_count: usize,
    );

    fn apply_candidate_terminal_plan_reason(
        &self,
        diagnostic: &mut Self::Diagnostic,
        no_plan_reason: &'static str,
    );

    fn record_candidate_skip_reason(
        &self,
        diagnostic: &mut Self::Diagnostic,
        skip_reason: &'static str,
    );

    fn set_runtime_miss_diagnostic(&self, trace_id: &str, diagnostic: Self::Diagnostic);

    fn mutate_runtime_miss_diagnostic<F>(&self, trace_id: &str, apply: F)
    where
        F: FnOnce(&mut Self::Diagnostic) + Send;

    fn runtime_miss_diagnostic_has_candidate_signal(&self, trace_id: &str) -> bool;
}

pub trait AiRuntimeMissDiagnosticFields {
    fn set_reason(&mut self, reason: String);
    fn set_candidate_count(&mut self, candidate_count: usize);
    fn candidate_count(&self) -> Option<usize>;
    fn skipped_candidate_count(&self) -> Option<usize>;
    fn skip_reason_count(&self, skip_reason: &str) -> usize;
    fn skip_reason_len(&self) -> usize;
    fn record_skip_reason(&mut self, skip_reason: &'static str);
}

pub fn apply_ai_runtime_candidate_evaluation_progress_to_diagnostic<Diagnostic>(
    diagnostic: &mut Diagnostic,
    candidate_count: usize,
) where
    Diagnostic: AiRuntimeMissDiagnosticFields,
{
    diagnostic.set_candidate_count(candidate_count);
    diagnostic.set_reason(if candidate_count == 0 {
        "candidate_list_empty".to_string()
    } else {
        "candidate_evaluation_incomplete".to_string()
    });
}

pub fn apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic<Diagnostic>(
    diagnostic: &mut Diagnostic,
    no_plan_reason: &'static str,
) where
    Diagnostic: AiRuntimeMissDiagnosticFields,
{
    let candidate_count = diagnostic.candidate_count().unwrap_or(0);
    let skipped_candidate_count = diagnostic.skipped_candidate_count().unwrap_or(0);
    diagnostic.set_reason(if candidate_count == 0 {
        "candidate_list_empty".to_string()
    } else if skipped_candidate_count >= candidate_count
        && diagnostic.skip_reason_len() == 1
        && diagnostic.skip_reason_count("api_key_concurrency_limit_reached") > 0
    {
        "api_key_concurrency_limit_reached".to_string()
    } else if skipped_candidate_count >= candidate_count {
        "all_candidates_skipped".to_string()
    } else {
        no_plan_reason.to_string()
    });
}

pub fn record_ai_runtime_candidate_skip_reason_on_diagnostic<Diagnostic>(
    diagnostic: &mut Diagnostic,
    skip_reason: &'static str,
) where
    Diagnostic: AiRuntimeMissDiagnosticFields,
{
    diagnostic.record_skip_reason(skip_reason);
}

pub fn set_ai_runtime_miss_diagnostic_reason<Port>(
    port: &Port,
    trace_id: &str,
    decision: &Port::Decision,
    plan_kind: &str,
    requested_model: Option<&str>,
    reason: &str,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    port.set_runtime_miss_diagnostic(
        trace_id,
        port.build_runtime_miss_diagnostic(decision, plan_kind, requested_model, reason),
    );
}

pub fn build_ai_runtime_execution_exhausted_diagnostic<Port>(
    port: &Port,
    decision: &Port::Decision,
    plan_kind: &str,
    requested_model: Option<&str>,
    candidate_count: usize,
) -> Port::Diagnostic
where
    Port: AiRuntimeMissDiagnosticPort,
{
    let mut diagnostic = port.build_runtime_miss_diagnostic(
        decision,
        plan_kind,
        requested_model,
        "execution_runtime_candidates_exhausted",
    );
    port.set_candidate_count(&mut diagnostic, candidate_count);
    diagnostic
}

pub fn set_ai_runtime_execution_exhausted_diagnostic<Port>(
    port: &Port,
    trace_id: &str,
    decision: &Port::Decision,
    plan_kind: &str,
    requested_model: Option<&str>,
    candidate_count: usize,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    port.set_runtime_miss_diagnostic(
        trace_id,
        build_ai_runtime_execution_exhausted_diagnostic(
            port,
            decision,
            plan_kind,
            requested_model,
            candidate_count,
        ),
    );
}

pub fn build_ai_runtime_candidate_evaluation_diagnostic<Port>(
    port: &Port,
    decision: &Port::Decision,
    plan_kind: &str,
    requested_model: Option<&str>,
    candidate_count: usize,
) -> Port::Diagnostic
where
    Port: AiRuntimeMissDiagnosticPort,
{
    let mut diagnostic = port.build_runtime_miss_diagnostic(
        decision,
        plan_kind,
        requested_model,
        "candidate_evaluation_incomplete",
    );
    port.apply_candidate_evaluation_progress(&mut diagnostic, candidate_count);
    diagnostic
}

pub fn set_ai_runtime_candidate_evaluation_diagnostic<Port>(
    port: &Port,
    trace_id: &str,
    decision: &Port::Decision,
    plan_kind: &str,
    requested_model: Option<&str>,
    candidate_count: usize,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    port.set_runtime_miss_diagnostic(
        trace_id,
        build_ai_runtime_candidate_evaluation_diagnostic(
            port,
            decision,
            plan_kind,
            requested_model,
            candidate_count,
        ),
    );
}

pub fn apply_ai_runtime_candidate_evaluation_progress<Port>(
    port: &Port,
    trace_id: &str,
    candidate_count: usize,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    port.mutate_runtime_miss_diagnostic(trace_id, |diagnostic| {
        port.apply_candidate_evaluation_progress(diagnostic, candidate_count);
    });
}

pub fn apply_ai_runtime_candidate_evaluation_progress_preserving_candidate_signal<Port>(
    port: &Port,
    trace_id: &str,
    candidate_count: usize,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    let preserve_existing_candidate_signal =
        candidate_count == 0 && port.runtime_miss_diagnostic_has_candidate_signal(trace_id);
    if preserve_existing_candidate_signal {
        return;
    }
    apply_ai_runtime_candidate_evaluation_progress(port, trace_id, candidate_count);
}

pub fn apply_ai_runtime_candidate_terminal_reason<Port>(
    port: &Port,
    trace_id: &str,
    no_plan_reason: &'static str,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    port.mutate_runtime_miss_diagnostic(trace_id, |diagnostic| {
        port.apply_candidate_terminal_plan_reason(diagnostic, no_plan_reason);
    });
}

pub fn record_ai_runtime_candidate_skip_reason<Port>(
    port: &Port,
    trace_id: &str,
    skip_reason: &'static str,
) where
    Port: AiRuntimeMissDiagnosticPort,
{
    port.mutate_runtime_miss_diagnostic(trace_id, |diagnostic| {
        port.record_candidate_skip_reason(diagnostic, skip_reason);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestDecision {
        id: &'static str,
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct TestDiagnostic {
        decision_id: String,
        plan_kind: String,
        requested_model: Option<String>,
        reason: String,
        candidate_count: Option<usize>,
        terminal_reason: Option<&'static str>,
        skip_reasons: BTreeMap<&'static str, usize>,
    }

    #[derive(Default)]
    struct TestPort {
        diagnostics: Mutex<BTreeMap<String, TestDiagnostic>>,
    }

    impl AiRuntimeMissDiagnosticPort for TestPort {
        type Decision = TestDecision;
        type Diagnostic = TestDiagnostic;

        fn build_runtime_miss_diagnostic(
            &self,
            decision: &Self::Decision,
            plan_kind: &str,
            requested_model: Option<&str>,
            reason: &str,
        ) -> Self::Diagnostic {
            TestDiagnostic {
                decision_id: decision.id.to_string(),
                plan_kind: plan_kind.to_string(),
                requested_model: requested_model.map(str::to_string),
                reason: reason.to_string(),
                ..Default::default()
            }
        }

        fn set_candidate_count(&self, diagnostic: &mut Self::Diagnostic, candidate_count: usize) {
            diagnostic.candidate_count = Some(candidate_count);
        }

        fn apply_candidate_evaluation_progress(
            &self,
            diagnostic: &mut Self::Diagnostic,
            candidate_count: usize,
        ) {
            diagnostic.candidate_count = Some(candidate_count);
        }

        fn apply_candidate_terminal_plan_reason(
            &self,
            diagnostic: &mut Self::Diagnostic,
            no_plan_reason: &'static str,
        ) {
            diagnostic.terminal_reason = Some(no_plan_reason);
        }

        fn record_candidate_skip_reason(
            &self,
            diagnostic: &mut Self::Diagnostic,
            skip_reason: &'static str,
        ) {
            *diagnostic.skip_reasons.entry(skip_reason).or_insert(0) += 1;
        }

        fn set_runtime_miss_diagnostic(&self, trace_id: &str, diagnostic: Self::Diagnostic) {
            self.diagnostics
                .lock()
                .unwrap()
                .insert(trace_id.to_string(), diagnostic);
        }

        fn mutate_runtime_miss_diagnostic<F>(&self, trace_id: &str, apply: F)
        where
            F: FnOnce(&mut Self::Diagnostic) + Send,
        {
            let mut diagnostics = self.diagnostics.lock().unwrap();
            let diagnostic = diagnostics.entry(trace_id.to_string()).or_default();
            apply(diagnostic);
        }

        fn runtime_miss_diagnostic_has_candidate_signal(&self, trace_id: &str) -> bool {
            self.diagnostics
                .lock()
                .unwrap()
                .get(trace_id)
                .is_some_and(|diagnostic| diagnostic.candidate_count.unwrap_or_default() > 0)
        }
    }

    impl AiRuntimeMissDiagnosticFields for TestDiagnostic {
        fn set_reason(&mut self, reason: String) {
            self.reason = reason;
        }

        fn set_candidate_count(&mut self, candidate_count: usize) {
            self.candidate_count = Some(candidate_count);
        }

        fn candidate_count(&self) -> Option<usize> {
            self.candidate_count
        }

        fn skipped_candidate_count(&self) -> Option<usize> {
            self.skip_reasons.values().copied().sum::<usize>().into()
        }

        fn skip_reason_count(&self, skip_reason: &str) -> usize {
            self.skip_reasons.get(skip_reason).copied().unwrap_or(0)
        }

        fn skip_reason_len(&self) -> usize {
            self.skip_reasons.len()
        }

        fn record_skip_reason(&mut self, skip_reason: &'static str) {
            *self.skip_reasons.entry(skip_reason).or_insert(0) += 1;
        }
    }

    #[test]
    fn runtime_miss_builds_and_sets_execution_exhausted_diagnostic() {
        let port = TestPort::default();

        set_ai_runtime_execution_exhausted_diagnostic(
            &port,
            "trace-a",
            &TestDecision { id: "decision-a" },
            "openai_chat",
            Some("gpt-5"),
            3,
        );

        let diagnostic = port.diagnostics.lock().unwrap().get("trace-a").cloned();
        assert_eq!(
            diagnostic,
            Some(TestDiagnostic {
                decision_id: "decision-a".to_string(),
                plan_kind: "openai_chat".to_string(),
                requested_model: Some("gpt-5".to_string()),
                reason: "execution_runtime_candidates_exhausted".to_string(),
                candidate_count: Some(3),
                ..Default::default()
            })
        );
    }

    #[test]
    fn runtime_miss_preserves_candidate_signal_and_records_terminal_updates() {
        let port = TestPort::default();
        apply_ai_runtime_candidate_evaluation_progress(&port, "trace-a", 2);

        apply_ai_runtime_candidate_evaluation_progress_preserving_candidate_signal(
            &port, "trace-a", 0,
        );
        apply_ai_runtime_candidate_terminal_reason(&port, "trace-a", "no_local_sync_plans");
        record_ai_runtime_candidate_skip_reason(&port, "trace-a", "transport_missing");

        let diagnostic = port.diagnostics.lock().unwrap().get("trace-a").cloned();
        assert_eq!(
            diagnostic,
            Some(TestDiagnostic {
                candidate_count: Some(2),
                terminal_reason: Some("no_local_sync_plans"),
                skip_reasons: BTreeMap::from([("transport_missing", 1)]),
                ..Default::default()
            })
        );
    }

    #[test]
    fn runtime_miss_diagnostic_field_helpers_apply_candidate_reason_state_machine() {
        let mut diagnostic = TestDiagnostic::default();

        apply_ai_runtime_candidate_evaluation_progress_to_diagnostic(&mut diagnostic, 0);
        assert_eq!(diagnostic.candidate_count, Some(0));
        assert_eq!(diagnostic.reason, "candidate_list_empty");

        apply_ai_runtime_candidate_evaluation_progress_to_diagnostic(&mut diagnostic, 3);
        assert_eq!(diagnostic.candidate_count, Some(3));
        assert_eq!(diagnostic.reason, "candidate_evaluation_incomplete");

        record_ai_runtime_candidate_skip_reason_on_diagnostic(
            &mut diagnostic,
            "api_key_concurrency_limit_reached",
        );
        record_ai_runtime_candidate_skip_reason_on_diagnostic(
            &mut diagnostic,
            "api_key_concurrency_limit_reached",
        );
        apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic(
            &mut diagnostic,
            "no_local_sync_plans",
        );
        assert_eq!(diagnostic.reason, "no_local_sync_plans");

        diagnostic.candidate_count = Some(2);
        apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic(
            &mut diagnostic,
            "no_local_sync_plans",
        );
        assert_eq!(diagnostic.reason, "api_key_concurrency_limit_reached");

        record_ai_runtime_candidate_skip_reason_on_diagnostic(&mut diagnostic, "transport_missing");
        apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic(
            &mut diagnostic,
            "no_local_sync_plans",
        );
        assert_eq!(diagnostic.reason, "all_candidates_skipped");
    }
}
