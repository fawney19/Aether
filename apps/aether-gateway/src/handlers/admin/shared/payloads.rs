use serde::{de, de::DeserializeOwned, Deserialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub(crate) fn deserialize_optional_f64_from_number_or_string<'de, D>(
    deserializer: D,
) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| de::Error::custom("expected a finite number")),
        Some(Value::String(raw)) => raw
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| de::Error::custom("expected a finite number or numeric string")),
        Some(_) => Err(de::Error::custom(
            "expected a finite number or numeric string",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminJsonFieldState {
    Missing,
    Null,
    Present,
}

impl AdminJsonFieldState {
    pub(crate) fn is_present(self) -> bool {
        !matches!(self, Self::Missing)
    }

    pub(crate) fn is_null(self) -> bool {
        matches!(self, Self::Null)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AdminJsonObjectPatch {
    field_states: BTreeMap<String, AdminJsonFieldState>,
}

impl AdminJsonObjectPatch {
    fn from_object(raw_payload: &Map<String, Value>) -> Self {
        let field_states = raw_payload
            .iter()
            .map(|(key, value)| {
                let state = if value.is_null() {
                    AdminJsonFieldState::Null
                } else {
                    AdminJsonFieldState::Present
                };
                (key.clone(), state)
            })
            .collect();
        Self { field_states }
    }

    pub(crate) fn state(&self, field: &str) -> AdminJsonFieldState {
        self.field_states
            .get(field)
            .copied()
            .unwrap_or(AdminJsonFieldState::Missing)
    }

    pub(crate) fn contains(&self, field: &str) -> bool {
        self.state(field).is_present()
    }

    pub(crate) fn is_null(&self, field: &str) -> bool {
        self.state(field).is_null()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AdminTypedObjectPatch<T> {
    fields: AdminJsonObjectPatch,
    pub(crate) payload: T,
}

impl<T> AdminTypedObjectPatch<T>
where
    T: DeserializeOwned,
{
    pub(crate) fn from_object(raw_payload: Map<String, Value>) -> Result<Self, serde_json::Error> {
        let fields = AdminJsonObjectPatch::from_object(&raw_payload);
        let payload = serde_json::from_value(Value::Object(raw_payload))?;
        Ok(Self { fields, payload })
    }

    pub(crate) fn contains(&self, field: &str) -> bool {
        self.fields.contains(field)
    }

    pub(crate) fn is_null(&self, field: &str) -> bool {
        self.fields.is_null(field)
    }

    pub(crate) fn into_parts(self) -> (AdminJsonObjectPatch, T) {
        (self.fields, self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::{AdminJsonFieldState, AdminTypedObjectPatch};
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    struct ExamplePatchPayload {
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
    }

    #[test]
    fn admin_typed_object_patch_tracks_field_presence() {
        let patch = AdminTypedObjectPatch::<ExamplePatchPayload>::from_object(
            json!({
                "description": null,
                "enabled": true,
            })
            .as_object()
            .cloned()
            .expect("object"),
        )
        .expect("patch");

        assert_eq!(patch.payload.description, None);
        assert_eq!(patch.payload.enabled, Some(true));
        assert!(patch.contains("description"));
        assert!(patch.is_null("description"));
        assert!(patch.contains("enabled"));
        assert!(!patch.is_null("enabled"));
        assert_eq!(
            patch.into_parts().0.state("missing_field"),
            AdminJsonFieldState::Missing
        );
    }
}
