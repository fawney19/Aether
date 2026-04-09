use serde_json::Value;

use crate::ai_pipeline::adaptation::private_envelope::transform_provider_private_stream_line as transform_envelope_line;
use crate::ai_pipeline::{
    provider_adaptation_should_unwrap_stream_envelope, StreamingStandardFormatMatrix,
};
use crate::GatewayError;

#[derive(Default)]
pub(crate) struct StreamingStandardConversionState {
    matrix: StreamingStandardFormatMatrix,
}

impl StreamingStandardConversionState {
    pub(crate) fn transform_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<u8>, GatewayError> {
        let line = if should_unwrap_envelope(report_context) {
            transform_envelope_line(report_context, line)
                .map_err(|err| GatewayError::Internal(err.to_string()))?
        } else {
            line
        };
        if line.is_empty() {
            return Ok(Vec::new());
        }
        self.matrix
            .transform_line(report_context, line)
            .map_err(Into::into)
    }

    pub(crate) fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, GatewayError> {
        self.matrix.finish(report_context).map_err(Into::into)
    }
}

fn should_unwrap_envelope(report_context: &Value) -> bool {
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    provider_adaptation_should_unwrap_stream_envelope(envelope_name, provider_api_format)
}
