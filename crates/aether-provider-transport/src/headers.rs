pub fn should_skip_request_header(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "x-aether-execution-path"
            | "x-aether-dependency-reason"
            | "x-aether-control-execute-fallback"
            | "x-aether-rate-limit-preflight"
    )
}

pub fn should_skip_upstream_passthrough_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // Anthropic SDK (stainless) client metadata and Anthropic-specific headers
    // (anthropic-version / anthropic-beta / anthropic-dangerous-direct-browser-access / ...).
    // These are only meaningful when the upstream is Anthropic. The Claude Code
    // adapter re-reads what it needs directly from the original HeaderMap and
    // injects its own values *after* this filter runs, so stripping both prefixes
    // here prevents leakage to any other upstream (OpenAI/Gemini/Codex/...).
    if lower.starts_with("x-stainless-") || lower.starts_with("anthropic-") {
        return true;
    }
    matches!(
        lower.as_str(),
        "authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "accept-encoding"
            | "content-encoding"
            | "x-real-ip"
            | "x-real-proto"
            | "x-forwarded-for"
            | "x-forwarded-proto"
            | "x-forwarded-scheme"
            | "x-forwarded-host"
            | "x-forwarded-port"
            // Claude CLI client identifier; re-injected by the Claude Code adapter
            // when the upstream is Anthropic, filtered for everybody else.
            | "x-app"
    ) || should_skip_request_header(name)
}

#[cfg(test)]
mod tests {
    use super::should_skip_upstream_passthrough_header;

    #[test]
    fn strips_all_stainless_headers() {
        let stainless = [
            "x-stainless-arch",
            "x-stainless-lang",
            "x-stainless-os",
            "x-stainless-package-version",
            "x-stainless-retry-count",
            "x-stainless-runtime",
            "x-stainless-runtime-version",
            "x-stainless-timeout",
            "x-stainless-helper-method",
            "X-Stainless-Arch",
            "X-STAINLESS-FUTURE-HEADER",
        ];
        for h in stainless {
            assert!(
                should_skip_upstream_passthrough_header(h),
                "should skip {h}"
            );
        }
    }

    #[test]
    fn strips_anthropic_and_claude_cli_identity_headers() {
        let anthropic = [
            "anthropic-version",
            "anthropic-beta",
            "anthropic-dangerous-direct-browser-access",
            "Anthropic-Version",
            "ANTHROPIC-FUTURE-HEADER",
            "x-app",
            "X-App",
        ];
        for h in anthropic {
            assert!(
                should_skip_upstream_passthrough_header(h),
                "should skip {h}"
            );
        }
    }

    #[test]
    fn allows_normal_headers_through() {
        let allowed = ["user-agent", "accept", "content-type", "x-custom-header"];
        for h in allowed {
            assert!(
                !should_skip_upstream_passthrough_header(h),
                "should allow {h}"
            );
        }
    }
}
