#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum AiCandidatePersistencePolicyKind {
    StandardDecision,
    SameFormatProviderDecision,
    OpenAiChatDecision,
    OpenAiResponsesDecision,
    ImageDecision,
    GeminiFilesDecision,
    VideoDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiCandidatePersistencePolicySpec {
    pub available_error_context: &'static str,
    pub skipped_error_context: &'static str,
    pub record_runtime_miss_diagnostic: bool,
}

pub fn ai_candidate_persistence_policy_spec(
    kind: AiCandidatePersistencePolicyKind,
) -> AiCandidatePersistencePolicySpec {
    match kind {
        AiCandidatePersistencePolicyKind::StandardDecision => AiCandidatePersistencePolicySpec {
            available_error_context:
                "gateway local standard decision request candidate upsert failed",
            skipped_error_context:
                "gateway local standard decision failed to persist skipped candidate",
            record_runtime_miss_diagnostic: true,
        },
        AiCandidatePersistencePolicyKind::SameFormatProviderDecision => {
            AiCandidatePersistencePolicySpec {
                available_error_context:
                    "gateway local same-format decision request candidate upsert failed",
                skipped_error_context:
                    "gateway local same-format decision failed to persist skipped candidate",
                record_runtime_miss_diagnostic: true,
            }
        }
        AiCandidatePersistencePolicyKind::OpenAiChatDecision => AiCandidatePersistencePolicySpec {
            available_error_context:
                "gateway local openai chat decision request candidate upsert failed",
            skipped_error_context:
                "gateway local openai chat decision failed to persist skipped candidate",
            record_runtime_miss_diagnostic: true,
        },
        AiCandidatePersistencePolicyKind::OpenAiResponsesDecision => {
            AiCandidatePersistencePolicySpec {
                available_error_context:
                    "gateway local openai responses decision request candidate upsert failed",
                skipped_error_context:
                    "gateway local openai responses decision failed to persist skipped candidate",
                record_runtime_miss_diagnostic: true,
            }
        }
        AiCandidatePersistencePolicyKind::ImageDecision => AiCandidatePersistencePolicySpec {
            available_error_context:
                "gateway local openai image decision request candidate upsert failed",
            skipped_error_context:
                "gateway local openai image decision failed to persist skipped candidate",
            record_runtime_miss_diagnostic: false,
        },
        AiCandidatePersistencePolicyKind::GeminiFilesDecision => AiCandidatePersistencePolicySpec {
            available_error_context: "gateway local gemini files request candidate upsert failed",
            skipped_error_context: "gateway local gemini files failed to persist skipped candidate",
            record_runtime_miss_diagnostic: false,
        },
        AiCandidatePersistencePolicyKind::VideoDecision => AiCandidatePersistencePolicySpec {
            available_error_context: "gateway local video decision request candidate upsert failed",
            skipped_error_context:
                "gateway local video decision failed to persist skipped candidate",
            record_runtime_miss_diagnostic: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_policies_define_candidate_persistence_side_effects() {
        let standard = ai_candidate_persistence_policy_spec(
            AiCandidatePersistencePolicyKind::StandardDecision,
        );
        assert_eq!(
            standard.available_error_context,
            "gateway local standard decision request candidate upsert failed"
        );
        assert_eq!(
            standard.skipped_error_context,
            "gateway local standard decision failed to persist skipped candidate"
        );
        assert!(standard.record_runtime_miss_diagnostic);

        let same_format = ai_candidate_persistence_policy_spec(
            AiCandidatePersistencePolicyKind::SameFormatProviderDecision,
        );
        assert_eq!(
            same_format.available_error_context,
            "gateway local same-format decision request candidate upsert failed"
        );
        assert_eq!(
            same_format.skipped_error_context,
            "gateway local same-format decision failed to persist skipped candidate"
        );
        assert!(same_format.record_runtime_miss_diagnostic);

        let image =
            ai_candidate_persistence_policy_spec(AiCandidatePersistencePolicyKind::ImageDecision);
        assert_eq!(
            image.available_error_context,
            "gateway local openai image decision request candidate upsert failed"
        );
        assert!(!image.record_runtime_miss_diagnostic);
    }
}
