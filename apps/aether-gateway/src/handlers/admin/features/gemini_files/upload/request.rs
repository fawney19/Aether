use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
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
    let boundary = admin_gemini_files_multipart_boundary(content_type)?;
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

fn admin_gemini_files_multipart_boundary(content_type: &str) -> Result<String, String> {
    let normalized = content_type.trim();
    if !normalized
        .to_ascii_lowercase()
        .starts_with("multipart/form-data")
    {
        return Err("Content-Type 必须是 multipart/form-data".to_string());
    }
    for part in normalized.split(';').skip(1) {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("boundary") {
            continue;
        }
        let boundary = value.trim().trim_matches('"').trim();
        if !boundary.is_empty() {
            return Ok(boundary.to_string());
        }
    }
    Err("multipart boundary 缺失".to_string())
}

fn admin_gemini_files_extract_file_part(
    body: &[u8],
    boundary: &str,
) -> Result<(String, String, Vec<u8>), String> {
    let boundary_marker = format!("--{boundary}");
    let next_boundary_marker = format!("\r\n--{boundary}");
    let boundary_bytes = boundary_marker.as_bytes();
    let next_boundary_bytes = next_boundary_marker.as_bytes();

    let mut cursor = 0usize;
    while cursor < body.len() {
        if !body[cursor..].starts_with(boundary_bytes) {
            return Err("multipart body 格式无效".to_string());
        }
        cursor += boundary_bytes.len();
        if body[cursor..].starts_with(b"--") {
            break;
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
        let headers_text = std::str::from_utf8(&body[cursor..headers_end])
            .map_err(|_| "multipart part 头部编码无效".to_string())?;
        cursor = headers_end + 4;
        let Some(next_boundary_rel) =
            admin_gemini_files_find_subslice(&body[cursor..], next_boundary_bytes)
        else {
            return Err("multipart body 缺少结束边界".to_string());
        };
        let content_end = cursor + next_boundary_rel;
        let content = &body[cursor..content_end];
        cursor = content_end + 2;

        let Some((field_name, file_name, mime_type)) =
            admin_gemini_files_parse_part_headers(headers_text)
        else {
            continue;
        };
        if field_name != "file" {
            continue;
        }
        return Ok((
            file_name.unwrap_or_else(|| "uploaded-file".to_string()),
            mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
            content.to_vec(),
        ));
    }

    Err("multipart body 中缺少 file 字段".to_string())
}

fn admin_gemini_files_parse_part_headers(
    headers_text: &str,
) -> Option<(String, Option<String>, Option<String>)> {
    let mut field_name = None;
    let mut file_name = None;
    let mut mime_type = None;

    for line in headers_text.split("\r\n") {
        let Some((header_name, header_value)) = line.split_once(':') else {
            continue;
        };
        let header_name = header_name.trim();
        let header_value = header_value.trim();
        if header_name.eq_ignore_ascii_case("content-disposition") {
            for part in header_value.split(';').skip(1) {
                let Some((key, value)) = part.trim().split_once('=') else {
                    continue;
                };
                let key = key.trim();
                let value = value.trim().trim_matches('"').trim();
                if key.eq_ignore_ascii_case("name") && !value.is_empty() {
                    field_name = Some(value.to_string());
                } else if key.eq_ignore_ascii_case("filename") && !value.is_empty() {
                    file_name = Some(value.to_string());
                }
            }
        } else if header_name.eq_ignore_ascii_case("content-type") && !header_value.is_empty() {
            mime_type = Some(header_value.to_string());
        }
    }

    field_name.map(|field_name| (field_name, file_name, mime_type))
}

fn admin_gemini_files_find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.is_empty() || needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
