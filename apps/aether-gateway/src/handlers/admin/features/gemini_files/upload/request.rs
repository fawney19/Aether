use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::{
    find_multipart_boundary, find_multipart_boundary_after_crlf, parse_multipart_boundary,
    MAX_MULTIPART_PARTS, MAX_MULTIPART_PART_HEADER_BYTES,
};
use aether_data_contracts::repository::gemini_file_mappings::{
    GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS, GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS,
};
use axum::body::Bytes;
use base64::Engine as _;

#[derive(Debug, Clone)]
pub(super) struct AdminGeminiFilesUploadRequest {
    pub(super) display_name: String,
    pub(super) mime_type: String,
    pub(super) body_bytes: Vec<u8>,
    pub(super) body_bytes_b64: String,
}

pub(super) fn admin_gemini_files_parse_upload_request(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<AdminGeminiFilesUploadRequest, String> {
    let _ = state;
    let content_type = request_context
        .content_type()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Content-Type 缺失".to_string())?;
    let boundary = parse_multipart_boundary(content_type)
        .ok_or_else(|| "multipart boundary 缺失或无效".to_string())?;
    let body = request_body
        .filter(|body| !body.is_empty())
        .ok_or_else(|| "上传文件不能为空".to_string())?;
    let (display_name, mime_type, body_bytes) =
        admin_gemini_files_extract_file_part(body.as_ref(), &boundary)?;
    Ok(AdminGeminiFilesUploadRequest {
        display_name,
        mime_type,
        body_bytes_b64: base64::engine::general_purpose::STANDARD.encode(&body_bytes),
        body_bytes,
    })
}

fn admin_gemini_files_extract_file_part(
    body: &[u8],
    boundary: &str,
) -> Result<(String, String, Vec<u8>), String> {
    let boundary_marker = format!("--{boundary}");
    let boundary_bytes = boundary_marker.as_bytes();

    let mut cursor = 0usize;
    let mut part_count = 0usize;
    let mut file_part = None;
    while cursor < body.len() {
        if find_multipart_boundary(&body[cursor..], boundary_bytes) != Some(0) {
            return Err("multipart body 格式无效".to_string());
        }
        cursor += boundary_bytes.len();
        if body[cursor..].starts_with(b"--") {
            let closing_suffix = body.get(cursor + 2..).unwrap_or_default();
            if !(closing_suffix.is_empty() || closing_suffix.starts_with(b"\r\n")) {
                return Err("multipart 结束边界格式无效".to_string());
            }
            break;
        }
        part_count = part_count.saturating_add(1);
        if part_count > MAX_MULTIPART_PARTS {
            return Err("multipart part 数量超过上限".to_string());
        }
        if !body[cursor..].starts_with(b"\r\n") {
            return Err("multipart body 缺少头部分隔符".to_string());
        }
        cursor += 2;
        let Some(headers_end_rel) = admin_gemini_files_find_subslice(&body[cursor..], b"\r\n\r\n")
        else {
            return Err("multipart part 缺少头部".to_string());
        };
        let headers_end = cursor + headers_end_rel;
        if headers_end_rel > MAX_MULTIPART_PART_HEADER_BYTES {
            return Err("multipart part 头部超过大小上限".to_string());
        }
        let headers_text = std::str::from_utf8(&body[cursor..headers_end])
            .map_err(|_| "multipart part 头部编码无效".to_string())?;
        cursor = headers_end + 4;
        let Some(next_boundary_rel) =
            find_multipart_boundary_after_crlf(&body[cursor..], boundary_bytes)
        else {
            return Err("multipart body 缺少结束边界".to_string());
        };
        let content_end = cursor + next_boundary_rel;
        // The CRLF immediately before the delimiter belongs to the
        // multipart framing, not to the uploaded file bytes.
        let content = body[cursor..content_end]
            .strip_suffix(b"\r\n")
            .unwrap_or(&body[cursor..content_end]);
        cursor = content_end;

        let Some((field_name, file_name, mime_type)) =
            admin_gemini_files_parse_part_headers(headers_text)
        else {
            return Err("multipart part 头部无效".to_string());
        };
        if field_name != "file" {
            continue;
        }
        if file_part.is_some() {
            return Err("multipart body 包含多个 file 字段".to_string());
        }
        file_part = Some((
            file_name.unwrap_or_else(|| "uploaded-file".to_string()),
            mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
            content.to_vec(),
        ));
    }

    let (display_name, mime_type, content) =
        file_part.ok_or_else(|| "multipart body 中缺少 file 字段".to_string())?;
    if display_name
        .chars()
        .nth(GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS)
        .is_some()
    {
        return Err("上传文件名超过长度上限".to_string());
    }
    if mime_type
        .chars()
        .nth(GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS)
        .is_some()
    {
        return Err("上传文件 Content-Type 超过长度上限".to_string());
    }
    Ok((display_name, mime_type, content))
}

fn admin_gemini_files_parse_part_headers(
    headers_text: &str,
) -> Option<(String, Option<String>, Option<String>)> {
    let mut field_name = None;
    let mut file_name = None;
    let mut mime_type = None;
    let mut disposition_seen = false;
    let mut content_type_seen = false;

    for line in headers_text.split("\r\n") {
        let (header_name, header_value) = line.split_once(':')?;
        let header_name = header_name.trim();
        let header_value = header_value.trim();
        if header_name.eq_ignore_ascii_case("content-disposition") {
            if disposition_seen {
                return None;
            }
            disposition_seen = true;
            let (name, filename) = admin_gemini_files_parse_content_disposition(header_value)?;
            field_name = Some(name);
            file_name = filename;
        } else if header_name.eq_ignore_ascii_case("content-type") {
            if content_type_seen
                || header_value.is_empty()
                || header_value.chars().any(char::is_control)
            {
                return None;
            }
            content_type_seen = true;
            mime_type = Some(header_value.to_string());
        }
    }

    field_name.map(|field_name| (field_name, file_name, mime_type))
}

fn admin_gemini_files_parse_content_disposition(value: &str) -> Option<(String, Option<String>)> {
    let segments = admin_gemini_files_split_header_parameters(value)?;
    if !segments.first()?.trim().eq_ignore_ascii_case("form-data") {
        return None;
    }

    let mut seen_keys = Vec::new();
    let mut name = None;
    let mut filename = None;
    for segment in segments.into_iter().skip(1) {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        let (raw_key, raw_value) = segment.split_once('=')?;
        let key = raw_key.trim();
        if key.is_empty()
            || !key
                .as_bytes()
                .iter()
                .copied()
                .all(admin_gemini_files_is_token_byte)
        {
            return None;
        }
        if seen_keys
            .iter()
            .any(|seen: &String| seen.eq_ignore_ascii_case(key))
        {
            return None;
        }
        seen_keys.push(key.to_ascii_lowercase());

        let parsed_value = admin_gemini_files_parse_parameter_value(raw_value.trim())?;
        if key.eq_ignore_ascii_case("name") {
            if parsed_value.is_empty() {
                return None;
            }
            name = Some(parsed_value);
        } else if key.eq_ignore_ascii_case("filename") {
            if !parsed_value.is_empty() {
                filename = Some(parsed_value);
            }
        }
    }

    Some((name?, filename))
}

fn admin_gemini_files_split_header_parameters(value: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;

    for (index, byte) in value.as_bytes().iter().copied().enumerate() {
        if in_quotes {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_quotes = false;
            }
        } else if byte == b'"' {
            in_quotes = true;
        } else if byte == b';' {
            segments.push(&value[start..index]);
            start = index + 1;
        }
    }

    if in_quotes || escaped {
        return None;
    }
    segments.push(&value[start..]);
    Some(segments)
}

fn admin_gemini_files_parse_parameter_value(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value.starts_with('"') {
        if value.len() < 2 || !value.ends_with('"') {
            return None;
        }
        let inner = &value[1..value.len() - 1];
        let mut parsed = String::with_capacity(inner.len());
        let mut escaped = false;
        for character in inner.chars() {
            if escaped {
                if character.is_control() {
                    return None;
                }
                parsed.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                if character == '"' || character.is_control() {
                    return None;
                }
                parsed.push(character);
            }
        }
        if escaped {
            return None;
        }
        return Some(parsed);
    }

    value
        .as_bytes()
        .iter()
        .copied()
        .all(admin_gemini_files_is_token_byte)
        .then(|| value.to_string())
}

fn admin_gemini_files_is_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
            | b'!'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    )
}

fn admin_gemini_files_find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.is_empty() || needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::gemini_file_mappings::{
        GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS, GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS,
    };

    use super::{
        admin_gemini_files_extract_file_part, MAX_MULTIPART_PARTS, MAX_MULTIPART_PART_HEADER_BYTES,
    };

    #[test]
    fn multipart_upload_rejects_metadata_beyond_storage_limits() {
        let boundary = "metadata-limits";
        let oversized_filename = "f".repeat(GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS + 1);
        let oversized_mime_type = "m".repeat(GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS + 1);

        for (filename, mime_type, expected) in [
            (
                oversized_filename.as_str(),
                "application/octet-stream",
                "上传文件名超过长度上限",
            ),
            (
                "payload.bin",
                oversized_mime_type.as_str(),
                "上传文件 Content-Type 超过长度上限",
            ),
        ] {
            let body = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {mime_type}\r\n\r\nfile-body\r\n--{boundary}--\r\n"
            );

            assert_eq!(
                admin_gemini_files_extract_file_part(body.as_bytes(), boundary),
                Err(expected.to_string())
            );
        }
    }

    #[test]
    fn multipart_upload_rejects_excessive_part_count() {
        let boundary = "bounded-parts";
        let mut body = Vec::new();
        for index in 0..(MAX_MULTIPART_PARTS + 1) {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"field-{index}\"\r\n\r\nvalue\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        assert_eq!(
            admin_gemini_files_extract_file_part(&body, boundary),
            Err("multipart part 数量超过上限".to_string())
        );
    }

    #[test]
    fn multipart_upload_rejects_oversized_part_headers() {
        let boundary = "bounded-header";
        let mut body =
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; x=\"")
                .into_bytes();
        body.extend(std::iter::repeat_n(b'x', MAX_MULTIPART_PART_HEADER_BYTES));
        body.extend_from_slice(format!("\"\r\n\r\nfile-body\r\n--{boundary}--\r\n").as_bytes());

        assert_eq!(
            admin_gemini_files_extract_file_part(&body, boundary),
            Err("multipart part 头部超过大小上限".to_string())
        );
    }

    #[test]
    fn multipart_upload_preserves_boundary_like_payload() {
        let boundary = "payload-boundary";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"file\"; filename=\"payload.bin\"\r\n",
                "Content-Type: application/octet-stream\r\n\r\n",
                "prefix\r\n--{boundary}X\r\nsuffix--{boundary}\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );

        let (_, mime_type, content) =
            admin_gemini_files_extract_file_part(body.as_bytes(), boundary).expect("file part");
        assert_eq!(mime_type, "application/octet-stream");
        assert_eq!(
            content,
            format!("prefix\r\n--{boundary}X\r\nsuffix--{boundary}").into_bytes()
        );
    }

    #[test]
    fn multipart_upload_rejects_invalid_suffix_without_closing_boundary() {
        let boundary = "invalid-suffix";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"file\"\r\n\r\n",
                "file-body\r\n--{boundary}X\r\n"
            ),
            boundary = boundary,
        );

        assert_eq!(
            admin_gemini_files_extract_file_part(body.as_bytes(), boundary),
            Err("multipart body 缺少结束边界".to_string())
        );
    }

    #[test]
    fn multipart_upload_rejects_garbage_after_closing_boundary() {
        let boundary = "closing-suffix";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"file\"\r\n\r\n",
                "file-body\r\n",
                "--{boundary}--junk"
            ),
            boundary = boundary,
        );

        assert!(admin_gemini_files_extract_file_part(body.as_bytes(), boundary).is_err());
    }

    #[test]
    fn multipart_upload_validates_parts_after_file_before_returning() {
        let boundary = "trailing-invalid";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"file\"\r\n\r\n",
                "file-body\r\n",
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"metadata\"\r\n\r\n",
                "metadata\r\n--{boundary}X\r\n"
            ),
            boundary = boundary,
        );

        assert_eq!(
            admin_gemini_files_extract_file_part(body.as_bytes(), boundary),
            Err("multipart body 缺少结束边界".to_string())
        );
    }

    #[test]
    fn multipart_upload_parses_quoted_parameters_without_filename_confusion() {
        let boundary = "quoted-parameters";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; filename=\"prefix; name=\\\"decoy\\\".bin\"; name=\"file\"\r\n",
                "Content-Type: application/octet-stream\r\n\r\n",
                "file-body\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );

        let (filename, _, content) =
            admin_gemini_files_extract_file_part(body.as_bytes(), boundary).expect("file part");
        assert_eq!(filename, "prefix; name=\"decoy\".bin");
        assert_eq!(content, b"file-body");
    }

    #[test]
    fn multipart_upload_rejects_filename_embedded_name_and_duplicate_parameters() {
        let boundary = "ambiguous-parameters";
        for content_disposition in [
            "form-data; filename=\"name=\\\"file\\\"\"",
            "form-data; name=\"file\"; name=\"metadata\"",
            "form-data; name=\"file\"; filename=\"unterminated",
        ] {
            let body = format!(
                concat!(
                    "--{boundary}\r\n",
                    "Content-Disposition: {content_disposition}\r\n\r\n",
                    "file-body\r\n",
                    "--{boundary}--\r\n"
                ),
                boundary = boundary,
                content_disposition = content_disposition,
            );
            assert_eq!(
                admin_gemini_files_extract_file_part(body.as_bytes(), boundary),
                Err("multipart part 头部无效".to_string()),
                "header should be rejected: {content_disposition}"
            );
        }
    }

    #[test]
    fn multipart_upload_rejects_duplicate_file_parts() {
        let boundary = "duplicate-file";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"file\"\r\n\r\n",
                "first\r\n",
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"file\"\r\n\r\n",
                "second\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );

        assert_eq!(
            admin_gemini_files_extract_file_part(body.as_bytes(), boundary),
            Err("multipart body 包含多个 file 字段".to_string())
        );
    }
}
