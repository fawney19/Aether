use crate::config::ServiceRuntimeConfig;
use axum::body::Body;
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::Response;

static METRICS_NAMESPACE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricLabel {
    pub key: &'static str,
    pub value: String,
}

impl MetricLabel {
    pub fn new(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricSample {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: MetricKind,
    pub value: u64,
    pub labels: Vec<MetricLabel>,
}

impl MetricSample {
    pub fn new(name: &'static str, help: &'static str, kind: MetricKind, value: u64) -> Self {
        Self {
            name,
            help,
            kind,
            value,
            labels: Vec::new(),
        }
    }

    pub fn with_labels(mut self, labels: Vec<MetricLabel>) -> Self {
        self.labels = labels;
        self
    }
}

pub fn init_metrics(config: ServiceRuntimeConfig) {
    let _ = METRICS_NAMESPACE.set(config.observability.metrics_namespace);
}

pub fn metrics_namespace() -> Option<&'static str> {
    METRICS_NAMESPACE.get().copied()
}

pub fn render_prometheus_text(samples: &[MetricSample]) -> String {
    let mut body = String::new();
    let namespace = metrics_namespace();

    for sample in samples {
        let metric_name = format_metric_name(namespace, sample.name);
        body.push_str(&format!("# HELP {} {}\n", metric_name, sample.help));
        body.push_str(&format!(
            "# TYPE {} {}\n",
            metric_name,
            match sample.kind {
                MetricKind::Counter => "counter",
                MetricKind::Gauge => "gauge",
            }
        ));
        body.push_str(&metric_name);
        if !sample.labels.is_empty() {
            body.push('{');
            for (index, label) in sample.labels.iter().enumerate() {
                if index > 0 {
                    body.push(',');
                }
                body.push_str(label.key);
                body.push_str("=\"");
                body.push_str(&escape_prometheus_label(&label.value));
                body.push('"');
            }
            body.push('}');
        }
        body.push(' ');
        body.push_str(&sample.value.to_string());
        body.push('\n');
    }

    body
}

pub fn prometheus_response(samples: &[MetricSample]) -> Response<Body> {
    let mut response = Response::new(Body::from(render_prometheus_text(samples)));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
}

pub fn service_up_sample(service: &'static str) -> MetricSample {
    MetricSample::new(
        "service_up",
        "Whether the service process is currently up.",
        MetricKind::Gauge,
        1,
    )
    .with_labels(vec![MetricLabel::new("service", service)])
}

fn format_metric_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(namespace) if !namespace.is_empty() => format!("{}_{}", namespace, name),
        _ => name.to_string(),
    }
}

fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{
        prometheus_response, render_prometheus_text, service_up_sample, MetricKind, MetricLabel,
        MetricSample,
    };
    use axum::body::to_bytes;

    #[test]
    fn renders_prometheus_samples_with_labels() {
        let text = render_prometheus_text(&[MetricSample::new(
            "queue_depth",
            "Current queue depth",
            MetricKind::Gauge,
            3,
        )
        .with_labels(vec![MetricLabel::new("queue", "proxy_writer")])]);

        assert!(text.contains("# HELP queue_depth Current queue depth"));
        assert!(text.contains("# TYPE queue_depth gauge"));
        assert!(text.contains("queue_depth{queue=\"proxy_writer\"} 3"));
    }

    #[test]
    fn escapes_prometheus_labels() {
        let text = render_prometheus_text(&[MetricSample::new(
            "errors_total",
            "Errors",
            MetricKind::Counter,
            1,
        )
        .with_labels(vec![MetricLabel::new("message", "bad\"line\nx")])]);

        assert!(text.contains("message=\"bad\\\"line\\nx\""));
    }

    #[tokio::test]
    async fn builds_prometheus_http_response() {
        let response = prometheus_response(&[service_up_sample("gateway")]);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        assert_eq!(
            content_type,
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let text = String::from_utf8(body.to_vec()).expect("body should be utf8");
        assert!(text.contains("service_up{service=\"gateway\"} 1"));
    }
}
