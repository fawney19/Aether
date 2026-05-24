use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireType {
    Varint = 0,
    Fixed64 = 1,
    Len = 2,
    Fixed32 = 5,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Varint(u64),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub number: u32,
    pub wire_type: WireType,
    pub value: FieldValue,
}

impl Field {
    pub fn bytes(&self) -> &[u8] {
        match &self.value {
            FieldValue::Bytes(bytes) => bytes.as_slice(),
            FieldValue::Varint(_) => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoError {
    message: String,
}

impl ProtoError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtoError {}

pub fn encode_varint(value: u64) -> Vec<u8> {
    let mut value = value;
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

fn decode_varint(buf: &[u8], offset: usize) -> Result<(u64, usize), ProtoError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut pos = offset;
    while pos < buf.len() {
        let byte = buf[pos];
        pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, pos - offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err(ProtoError::new("varint overflow"));
        }
    }
    Err(ProtoError::new("truncated varint"))
}

fn tag(field: u32, wire_type: WireType) -> Vec<u8> {
    encode_varint((u64::from(field) << 3) | wire_type as u64)
}

pub fn write_varint_field(field: u32, value: u64) -> Vec<u8> {
    let mut out = tag(field, WireType::Varint);
    out.extend(encode_varint(value));
    out
}

pub fn write_string_field(field: u32, value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = tag(field, WireType::Len);
    out.extend(encode_varint(bytes.len() as u64));
    out.extend(bytes);
    out
}

pub fn write_message_field(field: u32, value: &[u8]) -> Vec<u8> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut out = tag(field, WireType::Len);
    out.extend(encode_varint(value.len() as u64));
    out.extend(value);
    out
}

pub fn write_bool_field(field: u32, value: bool) -> Vec<u8> {
    if value {
        write_varint_field(field, 1)
    } else {
        Vec::new()
    }
}

pub fn parse_fields(buf: &[u8]) -> Result<Vec<Field>, ProtoError> {
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let (tag, tag_len) = decode_varint(buf, pos)?;
        pos += tag_len;
        let number = (tag >> 3) as u32;
        let wire_type = match tag & 0x07 {
            0 => WireType::Varint,
            1 => WireType::Fixed64,
            2 => WireType::Len,
            5 => WireType::Fixed32,
            other => {
                return Err(ProtoError::new(format!(
                    "unknown wire type {other} at offset {pos}"
                )))
            }
        };
        let value = match wire_type {
            WireType::Varint => {
                let (value, value_len) = decode_varint(buf, pos)?;
                pos += value_len;
                FieldValue::Varint(value)
            }
            WireType::Len => {
                let (len, len_len) = decode_varint(buf, pos)?;
                pos += len_len;
                let len = usize::try_from(len)
                    .map_err(|_| ProtoError::new("length-delimited field too large"))?;
                if pos + len > buf.len() {
                    return Err(ProtoError::new(format!(
                        "truncated len-delimited field {number} at offset {pos}"
                    )));
                }
                let bytes = buf[pos..pos + len].to_vec();
                pos += len;
                FieldValue::Bytes(bytes)
            }
            WireType::Fixed64 => {
                if pos + 8 > buf.len() {
                    return Err(ProtoError::new(format!("truncated fixed64 field {number}")));
                }
                let bytes = buf[pos..pos + 8].to_vec();
                pos += 8;
                FieldValue::Bytes(bytes)
            }
            WireType::Fixed32 => {
                if pos + 4 > buf.len() {
                    return Err(ProtoError::new(format!("truncated fixed32 field {number}")));
                }
                let bytes = buf[pos..pos + 4].to_vec();
                pos += 4;
                FieldValue::Bytes(bytes)
            }
        };
        fields.push(Field {
            number,
            wire_type,
            value,
        });
    }
    Ok(fields)
}

pub fn get_field(fields: &[Field], number: u32, wire_type: Option<WireType>) -> Option<&Field> {
    fields
        .iter()
        .find(|field| field.number == number && wire_type.is_none_or(|ty| field.wire_type == ty))
}

pub fn get_all_fields(fields: &[Field], number: u32) -> Vec<&Field> {
    fields
        .iter()
        .filter(|field| field.number == number)
        .collect()
}

pub fn get_varint(fields: &[Field], number: u32) -> Option<u64> {
    match get_field(fields, number, Some(WireType::Varint))?.value {
        FieldValue::Varint(value) => Some(value),
        FieldValue::Bytes(_) => None,
    }
}

pub fn get_string(fields: &[Field], number: u32) -> Option<String> {
    let field = get_field(fields, number, Some(WireType::Len))?;
    String::from_utf8(field.bytes().to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_varint_and_string_fields_like_windsurfapi() {
        assert_eq!(encode_varint(300), vec![0xac, 0x02]);
        assert_eq!(
            write_string_field(3, "abc"),
            vec![0x1a, 0x03, b'a', b'b', b'c']
        );
        assert_eq!(write_bool_field(2, false), Vec::<u8>::new());
        assert_eq!(write_bool_field(2, true), vec![0x10, 0x01]);
    }

    #[test]
    fn parses_repeated_len_delimited_fields() {
        let mut bytes = Vec::new();
        bytes.extend(write_string_field(1, "alpha"));
        bytes.extend(write_string_field(1, "beta"));
        bytes.extend(write_varint_field(2, 42));

        let fields = parse_fields(&bytes).expect("fields should parse");
        assert_eq!(get_all_fields(&fields, 1).len(), 2);
        assert_eq!(get_string(&fields, 1).as_deref(), Some("alpha"));
        assert_eq!(get_varint(&fields, 2), Some(42));
    }

    #[test]
    fn rejects_truncated_len_delimited_field() {
        let err = parse_fields(&[0x0a, 0x05, b'a']).expect_err("must reject truncated field");
        assert!(err.to_string().contains("truncated"));
    }
}
