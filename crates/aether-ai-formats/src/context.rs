use std::{error::Error, fmt};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    UnsupportedFormat(String),
    RequestParseFailed { format: String },
    RequestEmitFailed { format: String },
    ResponseParseFailed { format: String },
    ResponseEmitFailed { format: String },
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
        }
    }
}

impl Error for FormatError {}
