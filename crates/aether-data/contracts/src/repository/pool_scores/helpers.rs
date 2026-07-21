pub fn merge_score_reason_patch(
    mut current: serde_json::Value,
    patch: Option<serde_json::Value>,
) -> serde_json::Value {
    let Some(patch) = patch else {
        return current;
    };
    match (current.as_object_mut(), patch) {
        (Some(current), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                current.insert(key, value);
            }
            serde_json::Value::Object(current.clone())
        }
        (_, patch) => patch,
    }
}

pub fn score_with_delta(score: f64, delta_basis_points: Option<i32>) -> f64 {
    let delta = delta_basis_points.unwrap_or_default() as f64 / 10_000.0;
    (score + delta).clamp(0.0, 1.0)
}

pub fn i64_from_u64(value: u64, field: &str) -> Result<i64, crate::DataLayerError> {
    i64::try_from(value).map_err(|_| {
        crate::DataLayerError::InvalidInput(format!("{field} exceeds signed 64-bit range"))
    })
}

pub fn i64_opt_from_u64(
    value: Option<u64>,
    field: &str,
) -> Result<Option<i64>, crate::DataLayerError> {
    value.map(|value| i64_from_u64(value, field)).transpose()
}

pub fn u64_from_i64(value: i64, field: &str) -> Result<u64, crate::DataLayerError> {
    u64::try_from(value).map_err(|_| {
        crate::DataLayerError::UnexpectedValue(format!("{field} is negative: {value}"))
    })
}

pub fn u64_opt_from_i64(
    value: Option<i64>,
    field: &str,
) -> Result<Option<u64>, crate::DataLayerError> {
    value.map(|value| u64_from_i64(value, field)).transpose()
}
