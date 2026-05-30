use super::{
    classified, classified_with_request_auth_channel, is_claude_cli_request, is_gemini_cli_request,
    is_gemini_models_route, is_gemini_operation_route, ClassifiedRoute,
};

pub(super) fn classify_ai_public_route(
    method: &http::Method,
    normalized_path: &str,
    headers: &http::HeaderMap,
) -> Option<ClassifiedRoute> {
    if let Some(route) = classify_antigravity_v1internal_route(method, normalized_path) {
        Some(route)
    } else if method == http::Method::POST && normalized_path == "/v1/chat/completions" {
        Some(classified(
            "ai_public",
            "openai",
            "chat",
            "openai:chat",
            true,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/embeddings" {
        Some(classified(
            "ai_public",
            "openai",
            "embedding",
            "openai:embedding",
            true,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/rerank" {
        Some(classified(
            "ai_public",
            "openai",
            "rerank",
            "openai:rerank",
            true,
        ))
    } else if method == http::Method::POST
        && matches!(normalized_path, "/v1/responses" | "/v1/responses/compact")
    {
        if normalized_path.ends_with("/compact") {
            Some(classified(
                "ai_public",
                "openai",
                "responses:compact",
                "openai:responses:compact",
                true,
            ))
        } else {
            Some(classified(
                "ai_public",
                "openai",
                "responses",
                "openai:responses",
                true,
            ))
        }
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/v1/images/generations" | "/v1/images/edits"
        )
    {
        Some(classified(
            "ai_public",
            "openai",
            "image",
            "openai:image",
            true,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/messages/count_tokens" {
        Some(classified(
            "ai_public",
            "claude",
            "count_tokens",
            "claude:messages",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/messages" {
        let request_auth_channel = if is_claude_cli_request(headers) {
            "bearer_like"
        } else {
            "api_key"
        };
        Some(classified_with_request_auth_channel(
            "ai_public",
            "claude",
            "messages",
            request_auth_channel,
            "claude:messages",
            true,
        ))
    } else if normalized_path.starts_with("/v1/videos") {
        Some(classified(
            "ai_public",
            "openai",
            "video",
            "openai:video",
            true,
        ))
    } else if method == http::Method::POST && is_gemini_models_route(normalized_path) {
        if normalized_path.ends_with(":predictLongRunning") {
            Some(classified(
                "ai_public",
                "gemini",
                "video",
                "gemini:video",
                true,
            ))
        } else if normalized_path.ends_with(":embedContent")
            || normalized_path.ends_with(":batchEmbedContents")
        {
            Some(classified_with_request_auth_channel(
                "ai_public",
                "gemini",
                "embedding",
                "api_key",
                "gemini:embedding",
                true,
            ))
        } else if is_gemini_cli_request(headers) {
            Some(classified_with_request_auth_channel(
                "ai_public",
                "gemini",
                "generate_content",
                "bearer_like",
                "gemini:generate_content",
                true,
            ))
        } else {
            Some(classified_with_request_auth_channel(
                "ai_public",
                "gemini",
                "generate_content",
                "api_key",
                "gemini:generate_content",
                true,
            ))
        }
    } else if is_gemini_operation_method(method, normalized_path)
        && is_gemini_operation_route(normalized_path)
    {
        Some(classified(
            "ai_public",
            "gemini",
            "video",
            "gemini:video",
            true,
        ))
    } else if is_gemini_files_method(method, normalized_path) {
        Some(classified(
            "ai_public",
            "gemini",
            "files",
            "gemini:files",
            true,
        ))
    } else {
        None
    }
}

fn is_gemini_operation_method(method: &http::Method, normalized_path: &str) -> bool {
    method == http::Method::GET
        || (method == http::Method::POST && normalized_path.ends_with(":cancel"))
}

fn is_gemini_files_method(method: &http::Method, normalized_path: &str) -> bool {
    (method == http::Method::POST && normalized_path == "/upload/v1beta/files")
        || ((method == http::Method::GET || method == http::Method::DELETE)
            && normalized_path.starts_with("/v1beta/files"))
}

fn classify_antigravity_v1internal_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    if method != http::Method::POST {
        return None;
    }

    let action = normalized_path.strip_prefix("/v1internal:")?;
    let (route_kind, execution_runtime_candidate) = match action {
        "loadCodeAssist" => ("load_code_assist", false),
        "fetchAvailableModels" => ("fetch_available_models", false),
        "fetchUserInfo" => ("fetch_user_info", false),
        "fetchAdminControls" => ("fetch_admin_controls", false),
        "setUserSettings" => ("set_user_settings", false),
        "listExperiments" => ("list_experiments", false),
        "recordCodeAssistMetrics" => ("record_code_assist_metrics", false),
        "streamGenerateContent" => ("stream_generate_content", true),
        _ => return None,
    };

    Some(classified_with_request_auth_channel(
        "ai_public",
        "antigravity",
        route_kind,
        "bearer_like",
        "antigravity:v1internal",
        execution_runtime_candidate,
    ))
}
