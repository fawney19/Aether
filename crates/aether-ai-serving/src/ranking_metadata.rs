use aether_scheduler_core::SchedulerRankingOutcome;
use serde_json::{Map, Value};

pub fn append_ai_ranking_metadata_to_object(
    object: &mut Map<String, Value>,
    ranking: &SchedulerRankingOutcome,
) {
    object.insert(
        "ranking_mode".to_string(),
        Value::String(format!("{:?}", ranking.ranking_mode)),
    );
    object.insert(
        "priority_mode".to_string(),
        Value::String(format!("{:?}", ranking.priority_mode)),
    );
    object.insert(
        "ranking_index".to_string(),
        Value::Number(serde_json::Number::from(ranking.ranking_index as u64)),
    );
    object.insert(
        "priority_slot".to_string(),
        Value::Number(serde_json::Number::from(i64::from(ranking.priority_slot))),
    );
    if let Some(promoted_by) = ranking.promoted_by {
        object.insert(
            "promoted_by".to_string(),
            Value::String(promoted_by.to_string()),
        );
    }
    if let Some(demoted_by) = ranking.demoted_by {
        object.insert(
            "demoted_by".to_string(),
            Value::String(demoted_by.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_scheduler_core::{SchedulerPriorityMode, SchedulerRankingMode};
    use serde_json::json;

    #[test]
    fn ranking_metadata_appends_scheduler_outcome_fields() {
        let ranking = SchedulerRankingOutcome {
            original_index: 2,
            ranking_index: 1,
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            priority_slot: 7,
            promoted_by: Some("cached_affinity"),
            demoted_by: Some("cross_format"),
        };
        let mut object = Map::new();

        append_ai_ranking_metadata_to_object(&mut object, &ranking);

        assert_eq!(object.get("ranking_mode"), Some(&json!("CacheAffinity")));
        assert_eq!(object.get("priority_mode"), Some(&json!("Provider")));
        assert_eq!(object.get("ranking_index"), Some(&json!(1)));
        assert_eq!(object.get("priority_slot"), Some(&json!(7)));
        assert_eq!(object.get("promoted_by"), Some(&json!("cached_affinity")));
        assert_eq!(object.get("demoted_by"), Some(&json!("cross_format")));
    }
}
