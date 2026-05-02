use serde_json::{json, Value};

use super::super::KiroClaudeStreamState;

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
        let Some(idx) = self.thinking_block_index else {
            return Vec::new();
        };
        let mut events = vec![json!({
            "type": "content_block_delta",
            "index": idx,
            "delta": {"type": "thinking_delta", "thinking": ""}
        })];
        events.extend(self.close_block(idx));
        events
    }
}
