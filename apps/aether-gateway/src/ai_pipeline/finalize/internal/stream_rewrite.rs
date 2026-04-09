use serde_json::Value;

use crate::ai_pipeline::adaptation::private_envelope::transform_provider_private_stream_line as transform_envelope_line;
use crate::ai_pipeline::adaptation::KiroToClaudeCliStreamState;
use crate::ai_pipeline::finalize::standard::StreamingStandardConversionState;
use crate::ai_pipeline::{resolve_finalize_stream_rewrite_mode, FinalizeStreamRewriteMode};
use crate::GatewayError;

enum RewriteMode {
    EnvelopeUnwrap,
    Standard(StreamingStandardConversionState),
    KiroToClaudeCli(KiroToClaudeCliStreamState),
}

pub(crate) struct LocalStreamRewriter {
    report_context: Value,
    buffered: Vec<u8>,
    mode: RewriteMode,
}

pub(crate) fn maybe_build_local_stream_rewriter(
    report_context: Option<&Value>,
) -> Option<LocalStreamRewriter> {
    let report_context = report_context?;
    let mode = match resolve_finalize_stream_rewrite_mode(report_context)? {
        FinalizeStreamRewriteMode::EnvelopeUnwrap => RewriteMode::EnvelopeUnwrap,
        FinalizeStreamRewriteMode::Standard => {
            RewriteMode::Standard(StreamingStandardConversionState::default())
        }
        FinalizeStreamRewriteMode::KiroToClaudeCli => {
            RewriteMode::KiroToClaudeCli(KiroToClaudeCliStreamState::new(report_context))
        }
    };

    Some(LocalStreamRewriter {
        report_context: report_context.clone(),
        buffered: Vec::new(),
        mode,
    })
}

impl LocalStreamRewriter {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, GatewayError> {
        if let RewriteMode::KiroToClaudeCli(state) = &mut self.mode {
            return state.push_chunk(&self.report_context, chunk);
        }
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(line_end) = self.buffered.iter().position(|byte| *byte == b'\n') {
            let line = self.buffered.drain(..=line_end).collect::<Vec<_>>();
            output.extend(self.transform_line(line)?);
        }
        Ok(output)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<u8>, GatewayError> {
        if let RewriteMode::KiroToClaudeCli(state) = &mut self.mode {
            return state.finish(&self.report_context);
        }
        if self.buffered.is_empty() {
            match &mut self.mode {
                RewriteMode::Standard(state) => return state.finish(&self.report_context),
                RewriteMode::KiroToClaudeCli(_) => {}
                RewriteMode::EnvelopeUnwrap => {}
            }
            return Ok(Vec::new());
        }
        let line = std::mem::take(&mut self.buffered);
        let mut output = self.transform_line(line)?;
        match &mut self.mode {
            RewriteMode::Standard(state) => {
                output.extend(state.finish(&self.report_context)?);
            }
            RewriteMode::KiroToClaudeCli(_) => {}
            RewriteMode::EnvelopeUnwrap => {}
        }
        Ok(output)
    }

    fn transform_line(&mut self, line: Vec<u8>) -> Result<Vec<u8>, GatewayError> {
        match &mut self.mode {
            RewriteMode::EnvelopeUnwrap => transform_envelope_line(&self.report_context, line)
                .map_err(|err| GatewayError::Internal(err.to_string())),
            RewriteMode::Standard(state) => state.transform_line(&self.report_context, line),
            RewriteMode::KiroToClaudeCli(_) => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
#[path = "../tests_stream.rs"]
mod tests;
