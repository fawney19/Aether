use std::collections::BTreeMap;

use redis::streams::StreamReadReply;
use redis::{from_redis_value, Value as RedisValue, VerbatimFormat};

use super::{parse_reclaim_result, parse_stream_read_entries, RedisStreamEntry};
use crate::DataLayerError;

fn bulk(text: &str) -> RedisValue {
    RedisValue::BulkString(text.as_bytes().to_vec())
}

fn fields_reply(fields: Vec<(RedisValue, RedisValue)>, resp3: bool) -> RedisValue {
    if resp3 {
        RedisValue::Map(fields)
    } else {
        RedisValue::Array(
            fields
                .into_iter()
                .flat_map(|(key, value)| [key, value])
                .collect(),
        )
    }
}

fn read_reply(id: RedisValue, fields: RedisValue, resp3: bool) -> RedisValue {
    let entries = RedisValue::Array(vec![RedisValue::Array(vec![id, fields])]);
    if resp3 {
        RedisValue::Map(vec![(bulk("usage:events"), entries)])
    } else {
        RedisValue::Array(vec![RedisValue::Array(vec![bulk("usage:events"), entries])])
    }
}

fn reclaim_reply(id: RedisValue, fields: RedisValue) -> RedisValue {
    RedisValue::Array(vec![
        bulk("0-0"),
        RedisValue::Array(vec![RedisValue::Array(vec![id, fields])]),
        RedisValue::Nil,
    ])
}

fn original_read_parser(value: &RedisValue) -> Result<Vec<RedisStreamEntry>, DataLayerError> {
    let reply = from_redis_value::<StreamReadReply>(value).map_err(crate::error::redis_error)?;
    Ok(reply
        .keys
        .into_iter()
        .flat_map(|key| key.ids)
        .map(|entry| RedisStreamEntry {
            id: entry.id,
            fields: entry
                .map
                .into_iter()
                .filter_map(|(field, value)| {
                    from_redis_value::<String>(&value)
                        .ok()
                        .map(|value| (field, value))
                })
                .collect(),
        })
        .collect())
}

#[test]
fn owned_read_moves_large_payload_and_id_buffers_in_resp2_and_resp3() {
    for resp3 in [false, true] {
        let mut payload = Vec::with_capacity(512 * 1024);
        payload.extend_from_slice(b"{\"message\":\"");
        payload.resize(256 * 1024, b'x');
        payload.extend_from_slice(b"\",\"cache_read_input_tokens\":0}\n");
        let expected = payload.clone();
        let pointer = payload.as_ptr();
        let capacity = payload.capacity();
        let mut id = Vec::with_capacity(64);
        id.extend_from_slice(b"1710000000000-0");
        let id_pointer = id.as_ptr();
        let id_capacity = id.capacity();
        let fields = fields_reply(
            vec![(bulk("payload"), RedisValue::BulkString(payload))],
            resp3,
        );
        let parsed =
            parse_stream_read_entries(read_reply(RedisValue::BulkString(id), fields, resp3))
                .expect("read reply");
        assert_eq!(parsed.len(), 1);
        let body = &parsed[0].fields["payload"];
        assert_eq!(body.as_bytes(), expected);
        assert_eq!(
            body.as_ptr(),
            pointer,
            "the RESP payload allocation must be reused"
        );
        assert_eq!(body.capacity(), capacity);
        assert_eq!(parsed[0].id.as_ptr(), id_pointer);
        assert_eq!(parsed[0].id.capacity(), id_capacity);
    }
}

#[test]
fn owned_reclaim_moves_payload_and_all_id_buffers() {
    for resp3 in [false, true] {
        let mut payload = Vec::with_capacity(512 * 1024);
        payload.resize(256 * 1024, b'x');
        let pointer = payload.as_ptr();
        let capacity = payload.capacity();
        let mut id = Vec::with_capacity(64);
        id.extend_from_slice(b"1710000000000-0");
        let id_pointer = id.as_ptr();
        let mut next_id = Vec::with_capacity(64);
        next_id.extend_from_slice(b"1710000000001-0");
        let next_pointer = next_id.as_ptr();
        let mut deleted_id = Vec::with_capacity(64);
        deleted_id.extend_from_slice(b"1709999999999-0");
        let deleted_pointer = deleted_id.as_ptr();
        let fields = fields_reply(
            vec![(bulk("payload"), RedisValue::BulkString(payload))],
            resp3,
        );
        let parsed = parse_reclaim_result(RedisValue::Array(vec![
            RedisValue::BulkString(next_id),
            RedisValue::Array(vec![RedisValue::Array(vec![
                RedisValue::BulkString(id),
                fields,
            ])]),
            RedisValue::Array(vec![RedisValue::BulkString(deleted_id)]),
        ]))
        .expect("reclaim reply");
        let body = &parsed.entries[0].fields["payload"];
        assert_eq!(body.len(), 256 * 1024);
        assert!(body.bytes().all(|byte| byte == b'x'));
        assert_eq!(body.as_ptr(), pointer);
        assert_eq!(body.capacity(), capacity);
        assert_eq!(parsed.entries[0].id.as_ptr(), id_pointer);
        assert_eq!(parsed.next_start_id.as_ptr(), next_pointer);
        assert_eq!(parsed.deleted_ids[0].as_ptr(), deleted_pointer);
    }
}

#[test]
fn owned_read_matches_existing_redis_decoder_for_supported_reply_shapes() {
    let mut replies = vec![
        RedisValue::Nil,
        RedisValue::Array(vec![]),
        RedisValue::Map(vec![]),
    ];
    for resp3 in [false, true] {
        for fields in [
            RedisValue::Nil,
            fields_reply(
                vec![(
                    bulk("payload"),
                    bulk("{ \"text\": \"caf\u{00e9}\", \"n\": 0 }\n"),
                )],
                resp3,
            ),
            fields_reply(
                vec![
                    (bulk("payload"), RedisValue::Nil),
                    (bulk("count"), RedisValue::Int(0)),
                ],
                resp3,
            ),
            fields_reply(
                vec![(bulk("payload"), RedisValue::BulkString(vec![0xff]))],
                resp3,
            ),
            fields_reply(
                vec![
                    (bulk("payload"), bulk("first")),
                    (bulk("payload"), bulk("last")),
                    (bulk("invalid"), RedisValue::Boolean(false)),
                ],
                resp3,
            ),
            fields_reply(
                vec![(
                    bulk("payload"),
                    RedisValue::Attribute {
                        data: Box::new(bulk("annotated payload")),
                        attributes: vec![(bulk("encoding"), bulk("utf8"))],
                    },
                )],
                resp3,
            ),
        ] {
            replies.push(read_reply(bulk("1-0"), fields, resp3));
        }
        replies.push(read_reply(RedisValue::Int(42), RedisValue::Nil, resp3));
    }
    for reply in replies {
        let expected = original_read_parser(&reply).expect("baseline reply");
        assert_eq!(
            parse_stream_read_entries(reply).expect("owned reply"),
            expected
        );
    }
}

#[test]
fn owned_read_preserves_duplicate_overwrite_before_value_filtering() {
    for resp3 in [false, true] {
        let reply = read_reply(
            bulk("1-0"),
            fields_reply(
                vec![
                    (bulk("payload"), bulk("valid earlier payload")),
                    (bulk("payload"), RedisValue::BulkString(vec![0xff])),
                    (bulk("retry"), bulk("first")),
                    (bulk("retry"), bulk("last")),
                ],
                resp3,
            ),
            resp3,
        );
        let parsed = parse_stream_read_entries(reply)
            .expect("invalid values are filtered after deduplication");
        assert_eq!(
            parsed[0].fields,
            BTreeMap::from([("retry".to_string(), "last".to_string())])
        );
    }
}

#[test]
fn owned_read_keeps_invalid_id_key_and_shape_errors_in_redis_category() {
    for reply in [
        RedisValue::Int(7),
        RedisValue::Array(vec![RedisValue::Array(vec![bulk("stream")])]),
        read_reply(RedisValue::BulkString(vec![0xff]), RedisValue::Nil, false),
        read_reply(bulk("1-0"), RedisValue::Array(vec![bulk("orphan")]), false),
        read_reply(
            bulk("1-0"),
            fields_reply(
                vec![(RedisValue::BulkString(vec![0xff]), bulk("value"))],
                true,
            ),
            true,
        ),
    ] {
        assert!(matches!(
            original_read_parser(&reply),
            Err(DataLayerError::Redis(_))
        ));
        assert!(matches!(
            parse_stream_read_entries(reply),
            Err(DataLayerError::Redis(_))
        ));
    }
}

#[test]
fn owned_reclaim_preserves_string_types_nil_and_duplicate_fields() {
    for resp3 in [false, true] {
        let fields = fields_reply(
            vec![
                (bulk("payload"), bulk("first")),
                (bulk("payload"), bulk("{\"text\":\"caf\u{00e9}\"}\n")),
                (bulk("zero"), RedisValue::Int(0)),
                (bulk("double"), RedisValue::Double(1.5)),
                (
                    bulk("simple"),
                    RedisValue::SimpleString("simple".to_string()),
                ),
                (bulk("okay"), RedisValue::Okay),
                (
                    bulk("verbatim"),
                    RedisValue::VerbatimString {
                        format: VerbatimFormat::Text,
                        text: "verbatim".to_string(),
                    },
                ),
                (
                    bulk("attribute"),
                    RedisValue::Attribute {
                        data: Box::new(bulk("annotated")),
                        attributes: vec![],
                    },
                ),
            ],
            resp3,
        );
        let parsed =
            parse_reclaim_result(reclaim_reply(bulk("1-0"), fields)).expect("reclaim reply");
        assert_eq!(
            parsed.entries[0].fields,
            BTreeMap::from([
                (
                    "payload".to_string(),
                    "{\"text\":\"caf\u{00e9}\"}\n".to_string()
                ),
                ("zero".to_string(), "0".to_string()),
                ("double".to_string(), "1.5".to_string()),
                ("simple".to_string(), "simple".to_string()),
                ("okay".to_string(), "OK".to_string()),
                ("verbatim".to_string(), "verbatim".to_string()),
                ("attribute".to_string(), "annotated".to_string()),
            ])
        );
        assert!(parsed.deleted_ids.is_empty());
    }
    let parsed =
        parse_reclaim_result(reclaim_reply(bulk("1-0"), RedisValue::Nil)).expect("nil fields");
    assert!(parsed.entries[0].fields.is_empty());
    let parsed = parse_reclaim_result(RedisValue::Array(vec![bulk("0-0"), RedisValue::Nil]))
        .expect("nil entries");
    assert!(parsed.entries.is_empty());
    assert!(parsed.deleted_ids.is_empty());
}

#[test]
fn owned_reclaim_preserves_strict_validation_and_error_context() {
    for (reply, context) in [
        (
            RedisValue::Nil,
            "redis xautoclaim returned non-array payload",
        ),
        (
            RedisValue::Array(vec![bulk("0-0")]),
            "redis xautoclaim returned 1 top-level fields",
        ),
        (
            RedisValue::Array(vec![RedisValue::Nil, RedisValue::Nil]),
            "redis xautoclaim next_start_id",
        ),
        (
            RedisValue::Array(vec![bulk("0-0"), RedisValue::Int(1)]),
            "redis xautoclaim entries payload was not an array",
        ),
        (
            RedisValue::Array(vec![bulk("0-0"), RedisValue::Array(vec![RedisValue::Nil])]),
            "redis xautoclaim entry was not an array",
        ),
        (
            RedisValue::Array(vec![
                bulk("0-0"),
                RedisValue::Array(vec![RedisValue::Array(vec![])]),
            ]),
            "redis xautoclaim entry had 0 fields",
        ),
        (
            reclaim_reply(RedisValue::BulkString(vec![0xff]), RedisValue::Nil),
            "redis xautoclaim entry id",
        ),
        (
            reclaim_reply(bulk("1-0"), RedisValue::Array(vec![bulk("orphan")])),
            "redis xautoclaim entry fields expected an even number",
        ),
        (
            reclaim_reply(bulk("1-0"), RedisValue::Int(1)),
            "redis xautoclaim entry fields expected a redis array/map payload",
        ),
        (
            reclaim_reply(
                bulk("1-0"),
                fields_reply(
                    vec![(bulk("payload"), RedisValue::BulkString(vec![0xff]))],
                    false,
                ),
            ),
            "redis xautoclaim entry fields was not a string-compatible",
        ),
        (
            reclaim_reply(
                bulk("1-0"),
                fields_reply(
                    vec![(RedisValue::BulkString(vec![0xff]), bulk("value"))],
                    true,
                ),
            ),
            "redis xautoclaim entry fields was not a string-compatible",
        ),
        (
            RedisValue::Array(vec![bulk("0-0"), RedisValue::Nil, RedisValue::Int(1)]),
            "redis xautoclaim deleted_ids expected a redis array payload",
        ),
        (
            RedisValue::Array(vec![
                bulk("0-0"),
                RedisValue::Nil,
                RedisValue::Array(vec![RedisValue::Nil]),
            ]),
            "redis xautoclaim deleted_ids was not a string-compatible",
        ),
    ] {
        let error = parse_reclaim_result(reply).expect_err("invalid reclaim reply");
        let DataLayerError::UnexpectedValue(message) = error else {
            panic!("reclaim parse error must keep its classification: {error}");
        };
        assert!(
            message.starts_with(context),
            "expected {context}, got {message}"
        );
    }
    // Unlike read-group's filter, reclaim has always rejected an invalid value even if
    // a later duplicate would overwrite it. Keep that validation order.
    let reply = reclaim_reply(
        bulk("1-0"),
        fields_reply(
            vec![
                (bulk("payload"), RedisValue::Nil),
                (bulk("payload"), bulk("later valid payload")),
            ],
            false,
        ),
    );
    assert!(matches!(
        parse_reclaim_result(reply),
        Err(DataLayerError::UnexpectedValue(_))
    ));
}
