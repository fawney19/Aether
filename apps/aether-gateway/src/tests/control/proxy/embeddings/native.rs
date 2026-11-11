use super::*;

#[test]
fn gemini_native_vertex_batch_removes_item_models() {
    run_embedding_proxy_test("gemini_native_vertex_batch_removes_item_models", || {
        verify_native_embedding_request(true, true)
    });
}

#[test]
fn gemini_native_vertex_single_preserves_multimodal_parts() {
    run_embedding_proxy_test(
        "gemini_native_vertex_single_preserves_multimodal_parts",
        || verify_native_embedding_request(true, false),
    );
}

#[test]
fn gemini_native_developer_batch_preserves_item_models() {
    run_embedding_proxy_test(
        "gemini_native_developer_batch_preserves_item_models",
        || verify_native_embedding_request(false, true),
    );
}

async fn verify_native_embedding_request(vertex: bool, batch: bool) {
    let first = json!({
        "model": "models/gemini-embedding-2-preview",
        "content": {"parts": [
            {"text": "hello"},
            {"inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII="}}
        ]},
        "outputDimensionality": 128,
        "taskType": "RETRIEVAL_QUERY"
    });
    let request_body = if batch {
        json!({"requests": [first, {
            "model": "models/gemini-embedding-2-preview",
            "content": {"parts": [{"text": "world"}]},
            "outputDimensionality": 256,
            "taskType": "RETRIEVAL_DOCUMENT",
            "title": "Example document"
        }]})
    } else {
        first
    };
    let mut expected_body = request_body.clone();
    expected_body.as_object_mut().unwrap().remove("model");
    if vertex && batch {
        for item in expected_body["requests"].as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("model");
        }
    }
    let action = if batch {
        "batchEmbedContents"
    } else {
        "embedContent"
    };
    let expected_url = if vertex {
        format!("https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-embedding-2:{action}?key=sk-upstream-vertex-gemini-embedding")
    } else {
        format!("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview:{action}")
    };
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let expected_body = expected_body.clone();
            let expected_url = expected_url.clone();
            async move {
                assert_eq!(plan.client_api_format, "gemini:embedding");
                assert_eq!(plan.provider_api_format, "gemini:embedding");
                assert_eq!(plan.method, "POST");
                assert_eq!(plan.url, expected_url);
                assert!(!plan.stream);
                let actual_body = match plan.body.json_body.as_ref() {
                    Some(body) => body.clone(),
                    None => {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(plan.body.body_bytes_b64.as_deref().expect("原生请求体"))
                            .expect("请求体应能解码");
                        serde_json::from_slice(&bytes).expect("请求体应为 JSON")
                    }
                };
                assert_eq!(actual_body, expected_body);
                Json(if batch {
                    gemini_batch_embedding_execution_result(&plan)
                } else {
                    gemini_embedding_execution_result(&plan)
                })
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let state = if vertex {
        vertex_gemini_embedding_success_state(execution_runtime_url, "gemini:embedding")
    } else {
        gemini_embedding_success_state(execution_runtime_url, "gemini:embedding")
    };
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let api_key = if vertex {
        "sk-vertex-gemini-embedding-success"
    } else {
        "sk-gemini-embedding-success"
    };
    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1beta/models/gemini-embedding-2-preview:{action}"
        ))
        .header("x-goog-api-key", api_key)
        .json(&request_body)
        .send()
        .await
        .expect("原生 Embedding 请求应完成");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("响应应为 JSON");
    if batch {
        assert_eq!(payload["embeddings"].as_array().unwrap().len(), 2);
        assert_eq!(payload["embeddings"][0]["values"], json!([0.1, 0.2, 0.3]));
        assert_eq!(payload["embeddings"][1]["values"], json!([0.4, 0.5, 0.6]));
    } else {
        assert_eq!(payload["embedding"]["values"], json!([0.1, 0.2, 0.3]));
    }
    gateway_handle.abort();
    execution_runtime_handle.abort();
}
