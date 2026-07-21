use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct FormatContext {
    pub mapped_model: Option<String>,
    pub request_path: Option<String>,
    pub upstream_is_stream: bool,
    pub report_context: Option<Value>,
}

impl FormatContext {
    pub fn with_mapped_model(mut self, mapped_model: impl Into<String>) -> Self {
        self.mapped_model = Some(mapped_model.into());
        self
    }

    pub fn with_request_path(mut self, request_path: impl Into<String>) -> Self {
        self.request_path = Some(request_path.into());
        self
    }

    pub fn with_upstream_stream(mut self, upstream_is_stream: bool) -> Self {
        self.upstream_is_stream = upstream_is_stream;
        self
    }

    pub fn with_report_context(mut self, report_context: Value) -> Self {
        self.report_context = Some(report_context);
        self
    }

    pub fn without_runtime_request_edits(&self) -> Self {
        Self {
            mapped_model: None,
            request_path: self.request_path.clone(),
            upstream_is_stream: false,
            report_context: self.report_context.clone(),
        }
    }

    pub(crate) fn mapped_model_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.mapped_model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(fallback)
    }

    pub(crate) fn report_context_value(&self) -> Value {
        self.report_context.clone().unwrap_or_else(|| {
            json!({
                "mapped_model": self.mapped_model,
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConversionFieldStatus {
    Native,
    Mapped,
    ExtensionPreserved,
    Unaudited,
    Unsupported,
    InvalidEnum,
    LossyBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionFieldRecord {
    pub field: String,
    pub status: ConversionFieldStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ConversionFieldRecord {
    pub fn new(
        field: impl Into<String>,
        status: ConversionFieldStatus,
        detail: Option<String>,
    ) -> Self {
        Self {
            field: field.into(),
            status,
            detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionReport {
    pub source_format: String,
    pub target_format: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ConversionFieldRecord>,
}

impl ConversionReport {
    pub fn new(source_format: impl Into<String>, target_format: impl Into<String>) -> Self {
        Self {
            source_format: source_format.into(),
            target_format: target_format.into(),
            fields: Vec::new(),
        }
    }

    pub fn record(
        &mut self,
        field: impl Into<String>,
        status: ConversionFieldStatus,
        detail: Option<String>,
    ) {
        self.fields
            .push(ConversionFieldRecord::new(field, status, detail));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Converted<T> {
    pub value: T,
    pub report: ConversionReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    UnsupportedFormat(String),
    RequestParseFailed {
        format: String,
    },
    RequestEmitFailed {
        format: String,
    },
    ResponseParseFailed {
        format: String,
    },
    ResponseEmitFailed {
        format: String,
    },
    UnsupportedField {
        format: String,
        field: String,
        reason: String,
    },
    UnauditedField {
        source_format: String,
        target_format: String,
        field: String,
        reason: String,
    },
    InvalidEnumValue {
        format: String,
        field: String,
        value: String,
    },
    LossyConversionBlocked {
        source_format: String,
        target_format: String,
        field: String,
        reason: String,
    },
    InvalidTargetField {
        format: String,
        field: String,
        reason: String,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat(format) => write!(f, "unsupported AI format: {format}"),
            Self::RequestParseFailed { format } => {
                write!(f, "failed to parse {format} request")
            }
            Self::RequestEmitFailed { format } => write!(f, "failed to emit {format} request"),
            Self::ResponseParseFailed { format } => {
                write!(f, "failed to parse {format} response")
            }
            Self::ResponseEmitFailed { format } => {
                write!(f, "failed to emit {format} response")
            }
            Self::UnsupportedField {
                format,
                field,
                reason,
            } => {
                write!(f, "unsupported field {field} in {format}: {reason}")
            }
            Self::UnauditedField {
                source_format,
                target_format,
                field,
                reason,
            } => {
                write!(
                    f,
                    "unaudited field {field} in {source_format} cannot be converted to {target_format}: {reason}"
                )
            }
            Self::InvalidEnumValue {
                format,
                field,
                value,
            } => {
                write!(f, "invalid enum value {value:?} for {format}.{field}")
            }
            Self::LossyConversionBlocked {
                source_format,
                target_format,
                field,
                reason,
            } => {
                write!(
                    f,
                    "lossy conversion blocked from {source_format} to {target_format} at {field}: {reason}"
                )
            }
            Self::InvalidTargetField {
                format,
                field,
                reason,
            } => {
                write!(f, "invalid target field {field} for {format}: {reason}")
            }
        }
    }
}

impl Error for FormatError {}
