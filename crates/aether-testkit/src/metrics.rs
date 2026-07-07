use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrometheusSample {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub value: String,
}

pub async fn fetch_prometheus_samples(url: &str) -> Result<Vec<PrometheusSample>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|err| format!("failed to build metrics http client: {err}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("failed to fetch metrics from {url}: {err}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| format!("failed to read metrics body from {url}: {err}"))?;
    if !status.is_success() {
        return Err(format!("metrics endpoint {url} returned {status}: {body}"));
    }
    Ok(parse_prometheus_samples(&body))
}

pub fn parse_prometheus_samples(text: &str) -> Vec<PrometheusSample> {
    text.lines()
        .filter_map(parse_prometheus_line)
        .collect::<Vec<_>>()
}

pub fn find_metric_value_u64(
    samples: &[PrometheusSample],
    metric_name: &str,
    labels: &[(&str, &str)],
) -> Option<u64> {
    samples
        .iter()
        .find(|sample| {
            metric_name_matches(&sample.name, metric_name) && labels_match(sample, labels)
        })
        .and_then(|sample| sample.value.parse::<u64>().ok())
}

fn metric_name_matches(actual: &str, expected: &str) -> bool {
    actual == expected
        || actual
            .rsplit_once('_')
            .map(|(_, suffix)| suffix == expected)
            .unwrap_or(false)
        || actual.ends_with(&format!("_{expected}"))
}

fn labels_match(sample: &PrometheusSample, labels: &[(&str, &str)]) -> bool {
    labels
        .iter()
        .all(|(key, value)| sample.labels.get(*key).map(|current| current.as_str()) == Some(*value))
}

fn parse_prometheus_line(line: &str) -> Option<PrometheusSample> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (metric, value) = trimmed.rsplit_once(' ')?;
    let (name, labels) = if let Some((name, raw_labels)) = metric
        .split_once('{')
        .and_then(|(name, rest)| rest.strip_suffix('}').map(|labels| (name, labels)))
    {
        (name.to_string(), parse_labels(raw_labels))
    } else {
        (metric.to_string(), BTreeMap::new())
    };
    Some(PrometheusSample {
        name,
        labels,
        value: value.to_string(),
    })
}

fn parse_labels(raw: &str) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    for pair in split_label_pairs(raw) {
        if let Some((key, value)) = pair.split_once('=') {
            labels.insert(
                key.trim().to_string(),
                unescape_label_value(value.trim().trim_matches('"')),
            );
        }
    }
    labels
}

fn split_label_pairs(raw: &str) -> Vec<&str> {
    let mut pairs = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ',' if !in_string => {
                let pair = raw[start..index].trim();
                if !pair.is_empty() {
                    pairs.push(pair);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    let pair = raw[start..].trim();
    if !pair.is_empty() {
        pairs.push(pair);
    }
    pairs
}

fn unescape_label_value(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('\\') => output.push('\\'),
            Some('"') => output.push('"'),
            Some(next) => {
                output.push('\\');
                output.push(next);
            }
            None => output.push('\\'),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{find_metric_value_u64, parse_prometheus_samples};

    #[test]
    fn parses_prometheus_samples_with_labels() {
        let samples = parse_prometheus_samples(
            r#"
# HELP aether_gateway_concurrency_in_flight Current number of in-flight operations.
# TYPE aether_gateway_concurrency_in_flight gauge
aether_gateway_concurrency_in_flight{gate="gateway_requests"} 7
aether_gateway_concurrency_rejected_total{gate="gateway_requests"} 12
"#,
        );

        assert_eq!(
            find_metric_value_u64(
                &samples,
                "concurrency_in_flight",
                &[("gate", "gateway_requests")]
            ),
            Some(7)
        );
        assert_eq!(
            find_metric_value_u64(
                &samples,
                "concurrency_rejected_total",
                &[("gate", "gateway_requests")]
            ),
            Some(12)
        );
    }

    #[test]
    fn parses_quoted_label_values_containing_commas() {
        let samples = parse_prometheus_samples(
            r#"
metric_with_sql{rank="1",query_prefix="SELECT id, name, created_at FROM request_candidates",state="active"} 2
"#,
        );

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].labels.get("rank").map(String::as_str), Some("1"));
        assert_eq!(
            samples[0].labels.get("query_prefix").map(String::as_str),
            Some("SELECT id, name, created_at FROM request_candidates")
        );
        assert_eq!(
            samples[0].labels.get("state").map(String::as_str),
            Some("active")
        );
    }

    #[test]
    fn unescapes_quoted_label_values() {
        let samples = parse_prometheus_samples(
            r#"
metric_with_escape{message="bad\"line\nx\\y"} 1
"#,
        );

        assert_eq!(
            samples[0].labels.get("message").map(String::as_str),
            Some("bad\"line\nx\\y")
        );
    }
}
