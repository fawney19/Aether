use std::collections::BTreeMap;

use super::{
    AwsEventFrame, AwsHeaderValue, AwsHeaders, EventStreamDecoder, FrameParseError,
    MAX_BUFFER_SIZE, MAX_ERRORS, MAX_MESSAGE_SIZE,
};
use crate::provider_compat::kiro_stream::kiro_crc32 as crc32;

impl EventStreamDecoder {
    pub(super) fn feed(&mut self, data: &[u8]) -> Result<(), String> {
        if self.stopped || data.is_empty() {
            return Ok(());
        }
        let new_size = self.buffer.len() + data.len();
        if new_size > MAX_BUFFER_SIZE {
            self.stopped = true;
            return Err(format!(
                "buffer overflow: size={new_size} max={MAX_BUFFER_SIZE}"
            ));
        }
        self.buffer.extend_from_slice(data);
        Ok(())
    }

    pub(super) fn decode_available(&mut self) -> Result<Vec<AwsEventFrame>, String> {
        let mut out = Vec::new();
        if self.stopped {
            return Ok(out);
        }

        loop {
            match parse_frame(&self.buffer) {
                Ok(Some((frame, consumed))) => {
                    if consumed == 0 {
                        break;
                    }
                    out.push(frame);
                    self.buffer.drain(..consumed);
                    self.error_count = 0;
                }
                Ok(None) => break,
                Err(FrameParseError::Incomplete) => break,
                Err(FrameParseError::Invalid(message)) => {
                    self.error_count += 1;
                    if self.error_count >= MAX_ERRORS {
                        self.stopped = true;
                        return Err(message);
                    }
                    if self.buffer.is_empty() {
                        break;
                    }
                    self.buffer.drain(..1);
                }
            }
        }

        Ok(out)
    }
}

impl AwsHeaders {
    fn get_string(&self, name: &str) -> Option<&str> {
        match self.values.get(name) {
            Some(AwsHeaderValue::String(value)) => Some(value.as_str()),
            _ => None,
        }
    }

    pub(super) fn message_type(&self) -> Option<&str> {
        self.get_string(":message-type")
    }

    pub(super) fn event_type(&self) -> Option<&str> {
        self.get_string(":event-type")
    }

    pub(super) fn exception_type(&self) -> Option<&str> {
        self.get_string(":exception-type")
    }

    pub(super) fn error_code(&self) -> Option<&str> {
        self.get_string(":error-code")
    }
}

fn parse_frame(buffer: &[u8]) -> Result<Option<(AwsEventFrame, usize)>, FrameParseError> {
    if buffer.len() < 12 {
        return Ok(None);
    }
    let total_length = u32::from_be_bytes(buffer[0..4].try_into().expect("slice size")) as usize;
    let header_length = u32::from_be_bytes(buffer[4..8].try_into().expect("slice size")) as usize;
    let prelude_crc = u32::from_be_bytes(buffer[8..12].try_into().expect("slice size"));

    if total_length < 16 {
        return Err(FrameParseError::Invalid(format!(
            "message too small: length={total_length}"
        )));
    }
    if total_length > MAX_MESSAGE_SIZE {
        return Err(FrameParseError::Invalid(format!(
            "message too large: length={total_length}"
        )));
    }
    if buffer.len() < total_length {
        return Ok(None);
    }
    if crc32(&buffer[0..8]) != prelude_crc {
        return Err(FrameParseError::Invalid("prelude crc mismatch".to_string()));
    }
    let message_crc = u32::from_be_bytes(
        buffer[total_length - 4..total_length]
            .try_into()
            .expect("slice size"),
    );
    if crc32(&buffer[..total_length - 4]) != message_crc {
        return Err(FrameParseError::Invalid("message crc mismatch".to_string()));
    }

    let headers_start = 12;
    let headers_end = headers_start + header_length;
    if headers_end > total_length - 4 {
        return Err(FrameParseError::Invalid(
            "header length exceeds frame boundary".to_string(),
        ));
    }
    let headers = parse_headers(&buffer[headers_start..headers_end], header_length)?;
    let payload = buffer[headers_end..total_length - 4].to_vec();
    Ok(Some((AwsEventFrame { headers, payload }, total_length)))
}

fn parse_headers(data: &[u8], header_length: usize) -> Result<AwsHeaders, FrameParseError> {
    if data.len() < header_length {
        return Err(FrameParseError::Incomplete);
    }
    let mut values = BTreeMap::new();
    let mut offset = 0usize;
    while offset < header_length {
        ensure_header_bytes(data, offset, 1)?;
        let name_len = data[offset] as usize;
        offset += 1;
        if name_len == 0 {
            return Err(FrameParseError::Invalid(
                "header name length cannot be 0".to_string(),
            ));
        }
        ensure_header_bytes(data, offset, name_len)?;
        let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
        offset += name_len;

        ensure_header_bytes(data, offset, 1)?;
        let value_type = data[offset];
        offset += 1;

        let value = match value_type {
            0 => AwsHeaderValue::Ignored,
            1 => AwsHeaderValue::Ignored,
            2 => {
                ensure_header_bytes(data, offset, 1)?;
                let _ = i8::from_be_bytes([data[offset]]);
                offset += 1;
                AwsHeaderValue::Ignored
            }
            3 => {
                ensure_header_bytes(data, offset, 2)?;
                let _ = i16::from_be_bytes(data[offset..offset + 2].try_into().expect("slice"));
                offset += 2;
                AwsHeaderValue::Ignored
            }
            4 => {
                ensure_header_bytes(data, offset, 4)?;
                let _ = i32::from_be_bytes(data[offset..offset + 4].try_into().expect("slice"));
                offset += 4;
                AwsHeaderValue::Ignored
            }
            5 | 8 => {
                ensure_header_bytes(data, offset, 8)?;
                let _ = i64::from_be_bytes(data[offset..offset + 8].try_into().expect("slice"));
                offset += 8;
                AwsHeaderValue::Ignored
            }
            6 => {
                ensure_header_bytes(data, offset, 2)?;
                let length = u16::from_be_bytes(data[offset..offset + 2].try_into().expect("slice"))
                    as usize;
                offset += 2;
                ensure_header_bytes(data, offset, length)?;
                offset += length;
                AwsHeaderValue::Ignored
            }
            7 => {
                ensure_header_bytes(data, offset, 2)?;
                let length = u16::from_be_bytes(data[offset..offset + 2].try_into().expect("slice"))
                    as usize;
                offset += 2;
                ensure_header_bytes(data, offset, length)?;
                let out = String::from_utf8_lossy(&data[offset..offset + length]).to_string();
                offset += length;
                AwsHeaderValue::String(out)
            }
            9 => {
                ensure_header_bytes(data, offset, 16)?;
                offset += 16;
                AwsHeaderValue::Ignored
            }
            other => {
                return Err(FrameParseError::Invalid(format!(
                    "invalid header type: {other}"
                )));
            }
        };
        values.insert(name, value);
    }
    Ok(AwsHeaders { values })
}

fn ensure_header_bytes(data: &[u8], offset: usize, needed: usize) -> Result<(), FrameParseError> {
    let available = data.len().saturating_sub(offset);
    if available < needed {
        return Err(FrameParseError::Incomplete);
    }
    Ok(())
}
