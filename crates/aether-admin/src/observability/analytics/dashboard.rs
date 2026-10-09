use super::{envelope, metrics_value, parse_overview_query, OverviewRequest};
use aether_data_contracts::repository::usage::{
    StoredUsageAnalytics, StoredUsageDashboardAnalytics, UsageAnalyticsMetrics,
    UsageAnalyticsQuery, UsageAnalyticsRow, UsageAnalyticsView, UsageDashboardAnalyticsQuery,
};
use chrono::DateTime;
use serde_json::{json, Value};

pub fn parse_dashboard_query(raw: Option<&str>) -> Result<UsageDashboardAnalyticsQuery, String> {
    let mut timezone = None;
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if key != "timezone" {
            return Err(format!("unsupported dashboard query parameter: {key}"));
        }
        if timezone.replace(value.into_owned()).is_some() {
            return Err("duplicate query parameters are not supported".into());
        }
    }
    let query = UsageDashboardAnalyticsQuery {
        timezone: timezone.unwrap_or_else(|| "UTC".into()),
    };
    query.validate().map_err(|err| err.to_string())?;
    Ok(query)
}

pub fn parse_dashboard_charts_query(raw: Option<&str>) -> Result<OverviewRequest, String> {
    for (key, _) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if !matches!(key.as_ref(), "from" | "to" | "timezone" | "granularity") {
            return Err(format!(
                "unsupported dashboard charts query parameter: {key}"
            ));
        }
    }
    let mut request = parse_overview_query(raw, UsageAnalyticsView::DashboardCharts)?;
    request.query.limit = OVERVIEW_CHART_LIMIT;
    Ok(request)
}

const OVERVIEW_CHART_LIMIT: u32 = 10_000;

pub fn dashboard_charts_value(snapshot: &StoredUsageAnalytics) -> Value {
    // The chart read deliberately computes only its displayed metrics. Avoid
    // presenting unmeasured diagnostics as zero through the shared serializer.
    let chart_metrics = |metrics: &UsageAnalyticsMetrics| {
        let mut value = metrics_value(metrics);
        for field in [
            "input_tokens",
            "output_tokens",
            "usage_active_users",
            "slow_request_count",
            "unclassified_failure_count",
        ] {
            value[field] = Value::Null;
        }
        value["usage_source"] = json!("unknown");
        value["usage_source_counts"] = json!({
            "reported": 0, "estimated": 0, "mixed": 0, "unknown": metrics.request_count,
        });
        value
    };
    let rows = |items: &[UsageAnalyticsRow]| {
        items
            .iter()
            .map(|row| {
                let mut value = chart_metrics(&row.metrics);
                if snapshot.unrecoverable_bucket_count > 0 {
                    mark_incomplete_amounts(&mut value);
                }
                value["id"] = json!(row.id);
                value["label"] = json!(row.label);
                value["bucket_start"] = json!(row.bucket_start);
                value["unique_providers"] = json!(row.metrics.unique_providers);
                value
            })
            .collect::<Vec<_>>()
    };
    let mut summary = chart_metrics(&snapshot.summary);
    if snapshot.unrecoverable_bucket_count > 0 {
        mark_incomplete_amounts(&mut summary);
    }
    json!({
        "summary": summary,
        "series": rows(&snapshot.rows),
        "models": rows(&snapshot.model_rows),
        "providers": rows(&snapshot.provider_rows),
    })
}

pub fn dashboard_value(
    query: &UsageDashboardAnalyticsQuery,
    snapshot: &StoredUsageDashboardAnalytics,
) -> Result<Value, String> {
    let today = summary_value(query, &snapshot.today, &snapshot.today_from, &snapshot.to)?;
    let mut total = summary_value(
        query,
        &snapshot.total,
        snapshot.total_from.as_deref().unwrap_or(&snapshot.to),
        &snapshot.to,
    )?;
    total["meta"]["range"]["period"] = json!("all_time");
    total["meta"]["range"]["available_from"] = json!(snapshot.total_from);
    // Lifetime cards intentionally omit expensive historical diagnostics. Do not expose
    // their uncomputed defaults as measured zeroes.
    let total_fields = [
        "request_count",
        "total_tokens",
        "billable_amount",
        "enabled_users",
    ];
    total["data"]
        .as_object_mut()
        .expect("metrics are an object")
        .retain(|key, _| total_fields.contains(&key.as_str()));
    total["meta"]["available_metrics"] = json!(total_fields);
    let total_coverage = total["meta"]["coverage"]
        .as_object_mut()
        .expect("coverage is an object");
    total_coverage.remove("attribution_available_count");
    total_coverage.remove("classified_failure_count");
    if snapshot.history_complete == Some(false) {
        total["meta"]["coverage"]["status"] = json!("partial");
        mark_incomplete_amounts(&mut total["data"]);
    }
    Ok(json!({"today": today, "total": total, "history_complete": snapshot.history_complete}))
}

fn summary_value(
    query: &UsageDashboardAnalyticsQuery,
    snapshot: &StoredUsageAnalytics,
    from: &str,
    to: &str,
) -> Result<Value, String> {
    let timestamp = |value: &str| {
        let parsed = DateTime::parse_from_rfc3339(value)
            .map_err(|_| "invalid dashboard snapshot timestamp".to_string())?;
        u64::try_from(parsed.timestamp_millis())
            .map_err(|_| "invalid dashboard snapshot timestamp".to_string())
    };
    let request = OverviewRequest {
        query: UsageAnalyticsQuery {
            from_unix_ms: timestamp(from)?,
            to_unix_ms: timestamp(to)?,
            timezone: query.timezone.clone(),
            ..Default::default()
        },
        csv: false,
        amount_basis: "billable".into(),
    };
    let mut data = metrics_value(&snapshot.summary);
    if snapshot.unrecoverable_bucket_count > 0 {
        mark_incomplete_amounts(&mut data);
    }
    Ok(envelope(&request, snapshot, data))
}

fn mark_incomplete_amounts(data: &mut Value) {
    for key in [
        "rated_amount",
        "billable_amount",
        "quota_covered_amount",
        "wallet_consumed_amount",
        "wallet_debit_amount",
        "wallet_recharge_debit_amount",
        "wallet_gift_debit_amount",
        "wallet_overdraft_amount",
    ] {
        if data[key]["status"] == "known" {
            data[key]["status"] = json!("known_subtotal");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_accepts_only_one_valid_timezone() {
        assert_eq!(parse_dashboard_query(None).unwrap().timezone, "UTC");
        assert_eq!(
            parse_dashboard_query(Some("timezone=Asia%2FShanghai"))
                .unwrap()
                .timezone,
            "Asia/Shanghai"
        );
        for raw in [
            "timezone=invalid",
            "timezone=UTC&timezone=UTC",
            "timezone=UTC&from=2026-01-01T00:00:00Z",
            "user_id=employee-1",
            "model=test-model",
        ] {
            assert!(parse_dashboard_query(Some(raw)).is_err(), "{raw}");
        }
    }

    #[test]
    fn dashboard_charts_keeps_a_bounded_installation_range() {
        let range =
            "from=2026-09-01T00:00:00Z&to=2026-09-08T00:00:00Z&timezone=UTC&granularity=day";
        let request = parse_dashboard_charts_query(Some(range)).unwrap();
        assert_eq!(request.query.view, UsageAnalyticsView::DashboardCharts);
        assert_eq!(request.query.limit, 10_000);
        for suffix in [
            "&user_id=alice",
            "&model=test",
            "&limit=1",
            "&format=csv",
            "&timezone=UTC",
        ] {
            assert!(parse_dashboard_charts_query(Some(&format!("{range}{suffix}"))).is_err());
        }
    }

    #[test]
    fn dashboard_marks_known_missing_history_as_partial() {
        let mut snapshot = StoredUsageDashboardAnalytics {
            today_from: "2026-09-11T00:00:00Z".into(),
            total_from: Some("2024-01-01T00:00:00Z".into()),
            to: "2026-09-11T12:00:00Z".into(),
            history_complete: Some(false),
            ..Default::default()
        };
        snapshot.total.summary.billable_amount = Some("3.00000000".into());
        let value = dashboard_value(&parse_dashboard_query(None).unwrap(), &snapshot).unwrap();
        assert_eq!(value["total"]["meta"]["range"]["period"], "all_time");
        assert_eq!(value["total"]["meta"]["coverage"]["status"], "partial");
        assert_eq!(
            value["total"]["data"]["billable_amount"]["status"],
            "known_subtotal"
        );
        assert_eq!(value["today"]["meta"]["coverage"]["status"], "complete");
        assert_eq!(value["history_complete"], false);
    }

    #[test]
    fn dashboard_today_preserves_known_deleted_usage_coverage() {
        let mut snapshot = StoredUsageDashboardAnalytics {
            today_from: "2026-09-11T00:00:00Z".into(),
            to: "2026-09-11T12:00:00Z".into(),
            ..Default::default()
        };
        snapshot.today.unrecoverable_bucket_count = 1;
        snapshot.today.summary.billable_amount = Some("2.00000000".into());
        let value = dashboard_value(&parse_dashboard_query(None).unwrap(), &snapshot).unwrap();
        assert_eq!(value["today"]["meta"]["coverage"]["status"], "partial");
        assert_eq!(
            value["today"]["data"]["billable_amount"]["value"],
            "2.00000000"
        );
        assert_eq!(
            value["today"]["data"]["billable_amount"]["status"],
            "known_subtotal"
        );
    }

    #[test]
    fn dashboard_total_exposes_only_computed_cards_and_preserves_coverage() {
        let mut snapshot = StoredUsageDashboardAnalytics {
            today_from: "2026-09-11T00:00:00Z".into(),
            total_from: Some("2024-01-01T00:00:00Z".into()),
            to: "2026-09-11T12:00:00Z".into(),
            ..Default::default()
        };
        snapshot.today.summary.successful_request_count = 2;
        snapshot.today.summary.latency_p95_ms = Some(1200.0);
        snapshot.total.summary = aether_data_contracts::repository::usage::UsageAnalyticsMetrics {
            request_count: 3,
            total_tokens: 120,
            usage_available_count: 2,
            pricing_available_count: 2,
            settled_count: 2,
            allocation_available_count: 1,
            enabled_users: 4,
            billable_amount: Some("1.25000000".into()),
            ..Default::default()
        };
        let value = dashboard_value(&parse_dashboard_query(None).unwrap(), &snapshot).unwrap();
        assert_eq!(
            value["total"]["data"],
            json!({
                "request_count": 3,
                "total_tokens": 120,
                "enabled_users": 4,
                "billable_amount": {
                    "value": "1.25000000", "currency": "USD", "basis": "billable", "status": "known_subtotal"
                }
            })
        );
        assert_eq!(value["total"]["meta"]["coverage"]["status"], "partial");
        assert_eq!(
            value["total"]["meta"]["coverage"]["allocation_available_count"],
            1
        );
        assert_eq!(
            value["total"]["meta"]["coverage"]["usage_available_count"],
            2
        );
        assert!(value["total"]["meta"]["coverage"]
            .get("attribution_available_count")
            .is_none());
        assert!(value["total"]["meta"]["coverage"]
            .get("classified_failure_count")
            .is_none());
        assert_eq!(value["today"]["data"]["successful_request_count"], 2);
        assert_eq!(value["today"]["data"]["latency_ms"]["p95"], 1200.0);
    }

    #[test]
    fn dashboard_charts_marks_all_amounts_in_a_window_with_lost_history() {
        let mut snapshot = StoredUsageAnalytics {
            unrecoverable_bucket_count: 1,
            ..Default::default()
        };
        snapshot.summary.billable_amount = Some("2.00000000".into());
        snapshot.summary.request_count = 1;
        snapshot.summary.total_tokens = 42;
        snapshot.summary.usage_available_count = 1;
        let row = UsageAnalyticsRow {
            id: Some("model-1".into()),
            label: None,
            bucket_start: Some("2026-09-11T00:00:00Z".into()),
            metrics: snapshot.summary.clone(),
        };
        snapshot.rows.push(row.clone());
        snapshot.model_rows.push(row.clone());
        snapshot.provider_rows.push(row);
        let data = dashboard_charts_value(&snapshot);
        for metrics in [
            &data["summary"],
            &data["series"][0],
            &data["models"][0],
            &data["providers"][0],
        ] {
            assert_eq!(metrics["total_tokens"], 42);
            assert_eq!(metrics["usage_source"], "unknown");
            assert_eq!(metrics["usage_source_counts"]["unknown"], 1);
            for field in [
                "input_tokens",
                "output_tokens",
                "usage_active_users",
                "slow_request_count",
                "unclassified_failure_count",
            ] {
                assert!(metrics[field].is_null(), "{field} was not measured");
            }
        }
        assert_eq!(
            data["summary"]["billable_amount"]["status"],
            "known_subtotal"
        );
        for group in ["series", "models", "providers"] {
            assert_eq!(data[group][0]["billable_amount"]["value"], "2.00000000");
            assert_eq!(
                data[group][0]["billable_amount"]["status"],
                "known_subtotal"
            );
        }
    }
}
