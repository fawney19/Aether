mod actions;
mod conditions;
mod failover;
mod model;
mod mutations;
mod policy;
mod ranking;
mod trace;
mod validation;

pub use actions::{
    RoutingAction, RoutingHeaderPatch, RoutingJsonPatchOperation, RoutingRulePhase,
    RoutingSchedulingMode, RoutingSetPriorityMode,
};
pub use conditions::{RoutingCondition, RoutingConditionContext, RoutingConditionOp};
pub use failover::{
    validate_routing_failover_rules, RoutingFailoverRule, RoutingFailoverRules,
    MAX_ROUTING_FAILOVER_PATTERN_BYTES, MAX_ROUTING_FAILOVER_RULES,
};
pub use model::{
    RoutingDefaultPolicy, RoutingExecutionPolicy, RoutingGroupBinding, RoutingGroupBindingSubject,
    RoutingGroupConfig, RoutingGroupRecord, RoutingGroupVersionRecord, RoutingModelPolicy,
    RoutingPoolPolicyOverride, RoutingRule, RoutingSchedulingPreset, DEFAULT_STICKY_KEY_ATTEMPTS,
};
pub use mutations::{
    apply_json_patch_operations, validate_header_patch, validate_json_patch_operations,
    HeaderMutation, MutationError, MutationPlan,
};
pub use policy::{
    resolve_routing_policy, MatchedRoutingRule, ResolvedRoutingPolicy, RoutingPolicyError,
    RoutingPolicyInput,
};
pub use ranking::{
    rank_vector_for_candidate, CandidateKind, RankingOverlay, RoutingCandidateFacts,
    RoutingCandidateRankVector, ROUTING_PRIORITY_UNSPECIFIED,
};
pub use trace::{
    RoutingCandidateTrace, RoutingDecisionTrace, RoutingPatchSummary, RoutingPoolExpansionTrace,
    RoutingRuntimeFacts,
};
pub use validation::{
    validate_routing_group_config, RoutingValidationError, MAX_ROUTING_ALLOWED_KEYS,
};
