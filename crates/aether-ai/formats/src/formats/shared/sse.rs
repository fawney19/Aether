use serde_json::Value;

use crate::formats::shared::AiSurfaceFinalizeError;

#[derive(Debug, Default)]
pub struct SseRecord {
    pub event: Option<String>,
    pub data: Option<Vec<u8>>,
}

impl SseRecord {
    /// Adapt one complete event to the protocol parsers' logical data-line API.
    pub fn into_data_line(self) -> Option<Vec<u8>> {
        let mut data = self.data?;
        if let Some(event) = self.event {
            if let Ok(Value::Object(mut map)) = serde_json::from_slice(&data) {
                if !map.contains_key("type") {
                    map.insert("type".into(), Value::String(event));
                    data = serde_json::to_vec(&map).expect("JSON serialization");
                }
            }
        }
        let mut line = Vec::with_capacity(data.len() + 7);
        line.extend_from_slice(b"data: ");
        line.extend(data);
        line.push(b'\n');
        Some(line)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SseRecordTooLarge;

/// Bounded incremental SSE framing. After reporting an oversized record once,
/// discard through its blank separator and resume at the next record.
#[derive(Debug)]
pub struct SseRecordReader {
    max_bytes: usize,
    include_empty: bool,
    size: usize,
    line: Vec<u8>,
    record: SseRecord,
    skip_lf: bool,
    discarding: bool,
    nonempty_line: bool,
}

impl SseRecordReader {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            include_empty: false,
            size: 0,
            line: Vec::new(),
            record: SseRecord::default(),
            skip_lf: false,
            discarding: false,
            nonempty_line: false,
        }
    }

    /// Signal every record boundary so transparent adapters can also forward
    /// comments, retry hints and other fields without interpreting them.
    pub fn including_empty_records(mut self) -> Self {
        self.include_empty = true;
        self
    }

    pub fn next_record(
        &mut self,
        chunk: &mut &[u8],
    ) -> Option<Result<SseRecord, SseRecordTooLarge>> {
        while !chunk.is_empty() {
            if self.skip_lf {
                self.skip_lf = false;
                if chunk[0] == b'\n' {
                    *chunk = &chunk[1..];
                    continue;
                }
            }
            let end = chunk
                .iter()
                .position(|b| matches!(b, b'\r' | b'\n'))
                .unwrap_or(chunk.len());
            let (part, tail) = chunk.split_at(end);
            let separator = tail.first().copied();
            let width = match separator {
                Some(b'\r') if tail.get(1) == Some(&b'\n') => 2,
                Some(b'\r') => {
                    self.skip_lf = true;
                    1
                }
                Some(_) => 1,
                None => 0,
            };
            *chunk = &tail[width..];
            self.nonempty_line |= !part.is_empty();
            // Count logical newlines once, independent of CRLF chunk boundaries.
            self.size = self
                .size
                .saturating_add(end)
                .saturating_add(usize::from(separator.is_some()));
            let too_large = !self.discarding && self.size > self.max_bytes;
            if too_large {
                self.discarding = true;
                self.line = Vec::new();
                self.record = SseRecord::default();
            }
            if !self.discarding {
                self.line.extend_from_slice(part);
            }
            if separator.is_some() {
                if !self.nonempty_line {
                    self.size = 0;
                    let discard = std::mem::take(&mut self.discarding);
                    if !discard {
                        let record = std::mem::take(&mut self.record);
                        if self.include_empty || record.event.is_some() || record.data.is_some() {
                            return Some(Ok(record));
                        }
                    }
                } else if !self.discarding {
                    self.read_field();
                }
                self.nonempty_line = false;
            }
            if too_large {
                return Some(Err(SseRecordTooLarge));
            }
        }
        None
    }

    fn read_field(&mut self) {
        let (field, value) = match self.line.iter().position(|b| *b == b':') {
            Some(i) => (&self.line[..i], &self.line[i + 1..]),
            None => (self.line.as_slice(), &[][..]),
        };
        let value = value.strip_prefix(b" ").unwrap_or(value);
        match field {
            b"event" => {
                self.record.event = std::str::from_utf8(value)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            }
            b"data" => {
                if let Some(data) = &mut self.record.data {
                    data.push(b'\n');
                    data.extend_from_slice(value);
                } else {
                    self.record.data = Some(value.to_vec());
                }
            }
            _ => {}
        }
        self.line.clear();
    }

    /// Accept a final undelimited record for providers that close after the data.
    pub fn finish(&mut self) -> Option<SseRecord> {
        if !self.discarding && self.nonempty_line {
            self.read_field();
        }
        let record = std::mem::take(&mut self.record);
        let include_empty = self.include_empty;
        *self = Self::new(self.max_bytes);
        self.include_empty = include_empty;
        (record.event.is_some() || record.data.is_some()).then_some(record)
    }
}

pub fn map_claude_stop_reason(stop_reason: Option<&str>, has_tool_calls: bool) -> Option<String> {
    let mapped = match stop_reason {
        Some("end_turn") | Some("stop_sequence") => Some("stop".to_string()),
        Some("max_tokens") => Some("length".to_string()),
        Some("tool_use") => Some("tool_calls".to_string()),
        Some("pause_turn") => Some("stop".to_string()),
        Some(other) if !other.trim().is_empty() => Some(other.to_string()),
        _ => None,
    };
    if has_tool_calls && mapped.as_deref().is_none_or(|value| value == "stop") {
        Some("tool_calls".to_string())
    } else {
        mapped
    }
}

pub fn encode_done_sse() -> Vec<u8> {
    b"data: [DONE]\n\n".to_vec()
}

#[cfg(test)]
mod record_tests {
    use super::*;

    #[test]
    fn joins_data_across_every_chunk_boundary_and_flushes_last_record() {
        let wire = b":ignored\r\nevent: delta\rdata: {\r\ndata: \"x\":1}\r\n\r\ndata: [DONE]";
        for split in 0..=wire.len() {
            let mut reader = SseRecordReader::new(128);
            let mut events = Vec::new();
            for mut chunk in [&wire[..split], &wire[split..]] {
                while let Some(record) = reader.next_record(&mut chunk) {
                    events.push(record.unwrap());
                }
            }
            assert_eq!(events.len(), 1);
            let line = events.pop().unwrap().into_data_line().unwrap();
            let parsed: Value = serde_json::from_slice(&line[6..]).unwrap();
            assert_eq!(parsed, serde_json::json!({"type":"delta","x":1}));
            assert_eq!(reader.finish().unwrap().data.unwrap(), b"[DONE]");
            assert!(reader.finish().is_none());
        }
    }

    #[test]
    fn rejects_one_oversized_record_and_resumes_in_same_chunk() {
        let wire = b"data: oversized payload\r\ndata: [DONE]\r\n\r\ndata: ok\r\n\r\n";
        for width in 1..=wire.len() {
            let mut reader = SseRecordReader::new(12);
            let mut errors = 0;
            let mut output = Vec::new();
            for mut chunk in wire.chunks(width) {
                while let Some(record) = reader.next_record(&mut chunk) {
                    match record {
                        Ok(record) => output.push(record.data.unwrap()),
                        Err(_) => errors += 1,
                    }
                }
            }
            assert_eq!(errors, 1);
            assert_eq!(output, vec![b"ok".to_vec()]);
        }
    }
}

pub fn encode_json_sse(
    event: Option<&str>,
    value: &Value,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut out = Vec::new();
    if let Some(event) = event.filter(|value| !value.trim().is_empty()) {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(event.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"data: ");
    out.extend(serde_json::to_vec(value).map_err(AiSurfaceFinalizeError::from)?);
    out.extend_from_slice(b"\n\n");
    Ok(out)
}
