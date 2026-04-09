use serde_json::Value;

use crate::ai_pipeline::adaptation::KiroToClaudeCliStreamState;
use crate::ai_pipeline::{provider_adaptation_descriptor_for_envelope, KIRO_ENVELOPE_NAME};
use crate::GatewayError;

use super::transform_provider_private_stream_line;

enum ProviderPrivateStreamNormalizeMode {
    EnvelopeUnwrap,
    KiroToClaudeCli(KiroToClaudeCliStreamState),
}

pub(crate) struct ProviderPrivateStreamNormalizer {
    report_context: Value,
    buffered: Vec<u8>,
    mode: ProviderPrivateStreamNormalizeMode,
}

pub(crate) fn maybe_build_provider_private_stream_normalizer(
    report_context: Option<&Value>,
) -> Option<ProviderPrivateStreamNormalizer> {
    let report_context = report_context?;
    if !report_context
        .get("has_envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let descriptor =
        provider_adaptation_descriptor_for_envelope(envelope_name, provider_api_format)?;
    let mode = if descriptor
        .envelope_name
        .eq_ignore_ascii_case(KIRO_ENVELOPE_NAME)
    {
        ProviderPrivateStreamNormalizeMode::KiroToClaudeCli(KiroToClaudeCliStreamState::new(
            report_context,
        ))
    } else if descriptor.unwraps_response_envelope {
        ProviderPrivateStreamNormalizeMode::EnvelopeUnwrap
    } else {
        return None;
    };
    Some(ProviderPrivateStreamNormalizer {
        report_context: report_context.clone(),
        buffered: Vec::new(),
        mode,
    })
}

impl ProviderPrivateStreamNormalizer {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, GatewayError> {
        match &mut self.mode {
            ProviderPrivateStreamNormalizeMode::KiroToClaudeCli(state) => {
                state.push_chunk(&self.report_context, chunk)
            }
            ProviderPrivateStreamNormalizeMode::EnvelopeUnwrap => {
                self.buffered.extend_from_slice(chunk);
                let mut output = Vec::new();
                while let Some(line_end) = self.buffered.iter().position(|byte| *byte == b'\n') {
                    let line = self.buffered.drain(..=line_end).collect::<Vec<_>>();
                    output.extend(
                        transform_provider_private_stream_line(&self.report_context, line)
                            .map_err(|err| GatewayError::Internal(err.to_string()))?,
                    );
                }
                Ok(output)
            }
        }
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<u8>, GatewayError> {
        match &mut self.mode {
            ProviderPrivateStreamNormalizeMode::KiroToClaudeCli(state) => {
                state.finish(&self.report_context)
            }
            ProviderPrivateStreamNormalizeMode::EnvelopeUnwrap => {
                if self.buffered.is_empty() {
                    return Ok(Vec::new());
                }
                let line = std::mem::take(&mut self.buffered);
                transform_provider_private_stream_line(&self.report_context, line)
                    .map_err(|err| GatewayError::Internal(err.to_string()))
            }
        }
    }
}
