use http::{HeaderMap, Method, Uri};

#[derive(Debug, Clone)]
pub struct PublicRequestContext<Decision> {
    pub trace_id: String,
    pub request_method: Method,
    pub request_path: String,
    pub request_query_string: Option<String>,
    pub request_content_type: Option<String>,
    pub host_header: Option<String>,
    pub control_decision: Option<Decision>,
}

impl<Decision> PublicRequestContext<Decision> {
    pub fn from_request_parts(
        trace_id: impl Into<String>,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        control_decision: Option<Decision>,
    ) -> Self {
        let request_path = if uri.path().starts_with('/') {
            uri.path().to_string()
        } else {
            format!("/{}", uri.path())
        };

        Self {
            trace_id: trace_id.into(),
            request_method: method.clone(),
            request_path,
            request_query_string: uri.query().map(ToOwned::to_owned),
            request_content_type: header_value(headers, http::header::CONTENT_TYPE),
            host_header: header_value(headers, http::header::HOST),
            control_decision,
        }
    }

    pub fn request_path_and_query(&self) -> String {
        if let Some(query) = self
            .request_query_string
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            format!("{}?{query}", self.request_path)
        } else {
            self.request_path.clone()
        }
    }
}

fn header_value(headers: &HeaderMap, name: http::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::PublicRequestContext;

    #[test]
    fn builds_request_metadata_without_runtime_dependencies() {
        let uri: http::Uri = "/v1/models?limit=10".parse().unwrap();
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::HOST, "api.example.test".parse().unwrap());
        headers.insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        let context = PublicRequestContext::from_request_parts(
            "trace-1",
            &http::Method::GET,
            &uri,
            &headers,
            Some("decision"),
        );

        assert_eq!(context.request_path_and_query(), "/v1/models?limit=10");
        assert_eq!(context.host_header.as_deref(), Some("api.example.test"));
        assert_eq!(context.control_decision, Some("decision"));
    }
}
