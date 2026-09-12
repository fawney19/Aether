/// RFC 2046 limits a multipart boundary to at most 70 characters.  Using
/// HTTP token syntax here also keeps the value safe to embed in the byte
/// delimiter used by the lightweight parsers.
pub(crate) const MAX_MULTIPART_BOUNDARY_BYTES: usize = 70;
/// Keep multipart metadata bounded independently of the file payload size.
pub(crate) const MAX_MULTIPART_PARTS: usize = 128;
pub(crate) const MAX_MULTIPART_PART_HEADER_BYTES: usize = 64 * 1024;

/// Find a multipart delimiter at the beginning of a buffer or after CRLF.
/// The returned index points at the delimiter itself (not the preceding CRLF).
pub(crate) fn find_multipart_boundary(haystack: &[u8], delimiter: &[u8]) -> Option<usize> {
    find_multipart_boundary_inner(haystack, delimiter, true)
}

/// Find a multipart delimiter that is preceded by CRLF.  This variant is used
/// while scanning a part payload, where an apparent delimiter at byte zero is
/// payload data rather than a valid framing boundary.
pub(crate) fn find_multipart_boundary_after_crlf(
    haystack: &[u8],
    delimiter: &[u8],
) -> Option<usize> {
    find_multipart_boundary_inner(haystack, delimiter, false)
}

fn find_multipart_boundary_inner(
    haystack: &[u8],
    delimiter: &[u8],
    allow_start: bool,
) -> Option<usize> {
    if delimiter.is_empty() {
        return None;
    }
    if allow_start && multipart_boundary_is_valid_at(haystack, 0, delimiter) {
        return Some(0);
    }

    let window_len = delimiter.len().checked_add(2)?;
    haystack
        .windows(window_len)
        .enumerate()
        .find_map(|(index, window)| {
            if &window[..2] != b"\r\n" || &window[2..] != delimiter {
                return None;
            }
            let delimiter_index = index + 2;
            multipart_boundary_is_valid_at(haystack, delimiter_index, delimiter)
                .then_some(delimiter_index)
        })
}

fn multipart_boundary_is_valid_at(haystack: &[u8], index: usize, delimiter: &[u8]) -> bool {
    let suffix_start = index.checked_add(delimiter.len());
    let Some(suffix_start) = suffix_start else {
        return false;
    };
    if !haystack
        .get(index..)
        .is_some_and(|remaining| remaining.starts_with(delimiter))
    {
        return false;
    }
    let Some(suffix) = haystack.get(suffix_start..) else {
        return false;
    };
    if suffix.starts_with(b"\r\n") {
        return true;
    }
    suffix
        .strip_prefix(b"--")
        .is_some_and(|remaining| remaining.is_empty() || remaining.starts_with(b"\r\n"))
}

/// Extract and validate a multipart boundary parameter.
///
/// The parameter may use the usual optional surrounding quotes, but the
/// boundary value itself must be an ASCII HTTP token.  Rejecting malformed
/// values at the content-type boundary prevents parser ambiguity and bounds
/// the work performed by downstream delimiter scans.
pub(crate) fn parse_multipart_boundary(content_type: &str) -> Option<String> {
    let segments = split_multipart_header_parameters(content_type)?;
    let media_type = segments.first()?.trim();
    if !media_type.eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }

    let mut boundary = None;
    let mut seen_keys = Vec::new();
    for segment in segments.into_iter().skip(1) {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        let (raw_key, raw_value) = segment.split_once('=')?;
        let key = raw_key.trim();
        if key.is_empty() || !key.as_bytes().iter().copied().all(is_http_token_byte) {
            return None;
        }
        if seen_keys
            .iter()
            .any(|seen: &String| seen.eq_ignore_ascii_case(key))
        {
            return None;
        }
        seen_keys.push(key.to_ascii_lowercase());

        let (value, had_escape) = parse_multipart_parameter_value(raw_value.trim())?;
        if !key.eq_ignore_ascii_case("boundary") {
            continue;
        }
        // Keep boundary parsing deliberately narrower than generic quoted
        // parameter parsing: escaped boundary values are ambiguous across
        // HTTP stacks and are rejected here.
        if had_escape {
            return None;
        }
        if !is_valid_multipart_boundary(&value) {
            return None;
        }
        boundary = Some(value);
    }

    boundary
}

/// Split a semicolon-delimited HTTP header while honoring quoted strings.
/// Returning `None` for an unterminated quote or escape prevents a malformed
/// parameter from being reinterpreted by a downstream parser.
fn split_multipart_header_parameters(value: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;

    for (index, character) in value.char_indices() {
        if character.is_ascii_control() {
            return None;
        }
        if in_quotes {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_quotes = false;
            }
        } else if character == '"' {
            in_quotes = true;
        } else if character == ';' {
            segments.push(&value[start..index]);
            start = index + character.len_utf8();
        }
    }

    if in_quotes || escaped {
        return None;
    }
    segments.push(&value[start..]);
    Some(segments)
}

fn parse_multipart_parameter_value(value: &str) -> Option<(String, bool)> {
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
        let mut had_escape = false;
        for character in inner.chars() {
            if escaped {
                if character.is_ascii_control() {
                    return None;
                }
                parsed.push(character);
                escaped = false;
                had_escape = true;
            } else if character == '\\' {
                escaped = true;
            } else {
                if character == '"' || character.is_ascii_control() {
                    return None;
                }
                parsed.push(character);
            }
        }
        if escaped {
            return None;
        }
        return Some((parsed, had_escape));
    }

    value
        .as_bytes()
        .iter()
        .copied()
        .all(is_http_token_byte)
        .then(|| (value.to_string(), false))
}

fn is_valid_multipart_boundary(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MULTIPART_BOUNDARY_BYTES
        && value.as_bytes().iter().copied().all(is_http_token_byte)
}

fn is_http_token_byte(byte: u8) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{
        find_multipart_boundary, find_multipart_boundary_after_crlf, parse_multipart_boundary,
        MAX_MULTIPART_BOUNDARY_BYTES, MAX_MULTIPART_PARTS, MAX_MULTIPART_PART_HEADER_BYTES,
    };

    #[test]
    fn accepts_token_boundary_and_optional_quotes() {
        assert_eq!(
            parse_multipart_boundary("Multipart/Form-Data; boundary=----WebKitFormBoundaryabc123")
                .as_deref(),
            Some("----WebKitFormBoundaryabc123")
        );
        assert_eq!(
            parse_multipart_boundary("multipart/form-data; boundary=\"quoted-boundary\"")
                .as_deref(),
            Some("quoted-boundary")
        );
    }

    #[test]
    fn rejects_non_token_control_quote_and_oversized_boundaries() {
        for content_type in [
            "multipart/form-data; boundary=",
            "multipart/form-data; boundary=bad boundary",
            "multipart/form-data; boundary=\"bad;boundary\"",
            "multipart/form-data; boundary=bad\r\nvalue",
            "multipart/form-data; boundary=bad\"quote",
            "multipart/form-data; boundary=\"unterminated",
            "multipart/form-data; foo",
            "multipart/form-data; foo=\"unterminated; boundary=valid",
            "multipart/form-data; boundary=valid trailing",
        ] {
            assert!(
                parse_multipart_boundary(content_type).is_none(),
                "{content_type:?}"
            );
        }

        let oversized = "a".repeat(MAX_MULTIPART_BOUNDARY_BYTES + 1);
        assert!(
            parse_multipart_boundary(&format!("multipart/form-data; boundary={oversized}"))
                .is_none()
        );
    }

    #[test]
    fn rejects_duplicate_boundary_parameters() {
        for content_type in [
            "multipart/form-data; boundary=first; boundary=second",
            "multipart/form-data; boundary=first; BOUNDARY=second",
        ] {
            assert!(
                parse_multipart_boundary(content_type).is_none(),
                "duplicate boundary parameters must be rejected: {content_type}"
            );
        }
    }

    #[test]
    fn accepts_quoted_unknown_parameters_with_semicolons() {
        assert_eq!(
            parse_multipart_boundary(
                "multipart/form-data; note=\"semi;colon\"; boundary=quoted-token"
            )
            .as_deref(),
            Some("quoted-token")
        );
    }

    #[test]
    fn rejects_escaped_boundary_and_duplicate_unknown_parameters() {
        for content_type in [
            "multipart/form-data; boundary=\"escaped\\\"token\"",
            "multipart/form-data; note=one; NOTE=two; boundary=token",
            "multipart/form-data; note=\"unterminated; boundary=token",
            "multipart/form-data; note=\"closed\"trailing; boundary=token",
        ] {
            assert!(
                parse_multipart_boundary(content_type).is_none(),
                "malformed content type must be rejected: {content_type:?}"
            );
        }
    }

    #[test]
    fn rejects_non_multipart_media_types() {
        assert!(parse_multipart_boundary("application/json; boundary=abc").is_none());
        assert!(parse_multipart_boundary("x-multipart/form-data; boundary=abc").is_none());
    }

    #[test]
    fn multipart_metadata_limits_remain_bounded() {
        assert_eq!(MAX_MULTIPART_PARTS, 128);
        assert_eq!(MAX_MULTIPART_PART_HEADER_BYTES, 64 * 1024);
    }

    #[test]
    fn boundary_scanner_ignores_embedded_markers_and_invalid_suffixes() {
        let delimiter = b"--boundary";
        let payload = b"prefix\r\n--boundaryX\r\nmore\r\n--boundary\r\n";
        assert_eq!(
            find_multipart_boundary_after_crlf(payload, delimiter),
            Some(payload.len() - delimiter.len() - 2)
        );
        assert_eq!(
            find_multipart_boundary(b"payload--boundary\r\n", delimiter),
            None
        );
        assert_eq!(find_multipart_boundary(b"--boundaryX\r\n", delimiter), None);
        assert_eq!(
            find_multipart_boundary_after_crlf(b"payload\r\n--boundaryX\r\n", delimiter),
            None
        );
    }
}
