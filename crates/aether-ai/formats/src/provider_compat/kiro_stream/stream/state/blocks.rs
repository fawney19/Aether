use serde_json::{json, Value};

use super::super::KiroClaudeStreamState;

const KIRO_COMPAT_THINKING_SIGNATURE: &str = "kiro-compat-thinking-signature";

impl KiroClaudeStreamState {
    pub(super) fn ensure_text_block_open(&mut self) -> Vec<Value> {
        if let Some(idx) = self.text_block_index {
            if self
                .open_blocks
                .get(&idx)
                .map(|value| value == "text")
                .unwrap_or(false)
            {
                return Vec::new();
            }
        }
        let idx = self.next_block_index;
        self.next_block_index += 1;
        self.text_block_index = Some(idx);
        self.open_blocks.insert(idx, "text".to_string());
        vec![json!({
            "type": "content_block_start",
            "index": idx,
            "content_block": {"type": "text", "text": ""}
        })]
    }

    pub(super) fn ensure_thinking_block_open(&mut self) -> Vec<Value> {
        if let Some(idx) = self.thinking_block_index {
            if self
                .open_blocks
                .get(&idx)
                .map(|value| value == "thinking")
                .unwrap_or(false)
            {
                return Vec::new();
            }
        }
        let idx = self.next_block_index;
        self.next_block_index += 1;
        self.thinking_block_index = Some(idx);
        self.open_blocks.insert(idx, "thinking".to_string());
        vec![json!({
            "type": "content_block_start",
            "index": idx,
            "content_block": {"type": "thinking", "thinking": ""}
        })]
    }

    pub(super) fn close_block(&mut self, idx: usize) -> Vec<Value> {
        if self.open_blocks.remove(&idx).is_none() {
            return Vec::new();
        }
        vec![json!({"type": "content_block_stop", "index": idx})]
    }

    pub(super) fn close_text_block(&mut self) -> Vec<Value> {
        let Some(idx) = self.text_block_index.take() else {
            return Vec::new();
        };
        self.close_block(idx)
    }

    pub(super) fn emit_text_delta(&mut self, text: &str) -> Vec<Value> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut events = self.ensure_text_block_open();
        let idx = self.text_block_index.unwrap_or_default();
        events.push(json!({
            "type": "content_block_delta",
            "index": idx,
            "delta": {"type": "text_delta", "text": text}
        }));
        events
    }

    pub(super) fn emit_thinking_delta(&mut self, thinking: &str) -> Vec<Value> {
        if thinking.is_empty() {
            return Vec::new();
        }
        let mut events = self.ensure_thinking_block_open();
        let idx = self.thinking_block_index.unwrap_or_default();
        events.push(json!({
            "type": "content_block_delta",
            "index": idx,
            "delta": {"type": "thinking_delta", "thinking": thinking}
        }));
        events
    }

    pub(super) fn close_thinking_block(&mut self) -> Vec<Value> {
        let Some(idx) = self.thinking_block_index.take() else {
            return Vec::new();
        };
        if self
            .open_blocks
            .get(&idx)
            .map(|value| value != "thinking")
            .unwrap_or(true)
        {
            return Vec::new();
        }

        // Claude 客户端要求每个 thinking 块在 stop 前有非空 signature。只有 Kiro
        // 原生 reasoning 块能使用上游真实签名，标签式 thinking 始终使用兼容占位。
        let signature = if self.native_thinking_block_open {
            self.pending_thinking_signature
                .take()
                .filter(|signature| !signature.is_empty())
                .unwrap_or_else(|| KIRO_COMPAT_THINKING_SIGNATURE.to_string())
        } else {
            KIRO_COMPAT_THINKING_SIGNATURE.to_string()
        };
        self.native_thinking_block_open = false;
        let mut events = vec![json!({
            "type": "content_block_delta",
            "index": idx,
            "delta": {"type": "thinking_delta", "thinking": ""}
        })];
        events.push(json!({
            "type": "content_block_delta",
            "index": idx,
            "delta": {"type": "signature_delta", "signature": signature}
        }));
        events.extend(self.close_block(idx));
        events
    }
}
