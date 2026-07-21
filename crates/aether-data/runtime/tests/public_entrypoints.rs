use std::{
    fs,
    path::{Path, PathBuf},
};

const OLD_ENTRYPOINTS: &[&str] = &[
    "backends", "backfill", "export", "migrate", "mysql", "postgres", "redis", "sqlite",
];
const SELF_TEST_PATH: &str = "crates/aether-data/runtime/tests/public_entrypoints.rs";

#[test]
fn deprecated_aether_data_entrypoints_are_not_exposed_or_used() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("aether-data should live under crates/aether-data/runtime/");
    let mut violations = Vec::new();

    for relative_dir in ["apps", "crates"] {
        scan_rust_files(
            &workspace_root.join(relative_dir),
            workspace_root,
            &mut violations,
        );
    }

    assert!(
        violations.is_empty(),
        "deprecated aether-data entrypoints should not be exposed or used inside the workspace:\n{}",
        violations.join("\n")
    );
}

fn scan_rust_files(dir: &Path, workspace_root: &Path, violations: &mut Vec<String>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| panic!("failed to read {dir:?}: {err}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|err| panic!("failed to read entry under {dir:?}: {err}"))
            .path();

        if path.is_dir() {
            scan_rust_files(&path, workspace_root, violations);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        if should_skip_file(&path, workspace_root) {
            continue;
        }

        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"));
        collect_file_violations(&path, workspace_root, &source, violations);
    }
}

fn should_skip_file(path: &Path, workspace_root: &Path) -> bool {
    let relative = relative_path(path, workspace_root);
    relative == Path::new(SELF_TEST_PATH)
}

fn collect_file_violations(
    path: &Path,
    workspace_root: &Path,
    source: &str,
    violations: &mut Vec<String>,
) {
    let relative = relative_path(path, workspace_root);

    collect_alias_violations(&relative, source, "aether_data", violations);

    if relative.starts_with("crates/aether-data/runtime/src") {
        collect_alias_violations(&relative, source, "crate", violations);
        if relative == Path::new("crates/aether-data/runtime/src/lib.rs") {
            collect_public_module_violations(&relative, source, violations);
        }
    }
}

fn collect_alias_violations(
    relative: &Path,
    source: &str,
    prefix: &str,
    violations: &mut Vec<String>,
) {
    for entrypoint in OLD_ENTRYPOINTS {
        let direct = format!("{prefix}::{entrypoint}");
        for (byte_index, _) in source.match_indices(&direct) {
            push_violation(relative, source, byte_index, &direct, violations);
        }
    }

    collect_grouped_alias_violations(relative, source, prefix, violations);
}

fn collect_grouped_alias_violations(
    relative: &Path,
    source: &str,
    prefix: &str,
    violations: &mut Vec<String>,
) {
    let marker = format!("{prefix}::{{");
    let mut offset = 0;

    while let Some(local_start) = source[offset..].find(&marker) {
        let marker_start = offset + local_start;
        let body_start = marker_start + marker.len();
        let Some((body_end, body)) = grouped_import_body(source, body_start) else {
            break;
        };

        for segment in top_level_import_segments(body) {
            for entrypoint in OLD_ENTRYPOINTS {
                if import_matches(segment, entrypoint) {
                    let imported = format!("{prefix}::{entrypoint}");
                    push_violation(relative, source, marker_start, &imported, violations);
                }
            }
        }

        offset = body_end + 1;
    }
}

fn collect_public_module_violations(relative: &Path, source: &str, violations: &mut Vec<String>) {
    for entrypoint in OLD_ENTRYPOINTS {
        for marker in [format!("pub mod {entrypoint}"), format!("mod {entrypoint}")] {
            for (byte_index, _) in source.match_indices(&marker) {
                push_violation(relative, source, byte_index, &marker, violations);
            }
        }
    }
}

fn grouped_import_body(source: &str, body_start: usize) -> Option<(usize, &str)> {
    let mut depth = 1usize;
    for (relative_index, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let body_end = body_start + relative_index;
                    return Some((body_end, &source[body_start..body_end]));
                }
            }
            _ => {}
        }
    }
    None
}

fn top_level_import_segments(body: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut segment_start = 0usize;

    for (index, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                segments.push(&body[segment_start..index]);
                segment_start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    segments.push(&body[segment_start..]);
    segments
}

fn import_matches(part: &str, entrypoint: &str) -> bool {
    let trimmed = part.trim();
    trimmed == entrypoint
        || trimmed.strip_prefix(entrypoint).is_some_and(|suffix| {
            suffix.starts_with("::") || suffix.trim_start().starts_with("as ")
        })
}

fn push_violation(
    relative: &Path,
    source: &str,
    byte_index: usize,
    imported: &str,
    violations: &mut Vec<String>,
) {
    violations.push(format!(
        "{}:{}: deprecated {imported}",
        relative.display(),
        line_number(source, byte_index)
    ));
}

fn line_number(source: &str, byte_index: usize) -> usize {
    source[..byte_index]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn relative_path(path: &Path, workspace_root: &Path) -> PathBuf {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_path_buf()
}

#[test]
fn grouped_import_scanner_allows_nested_new_paths() {
    let source = r#"
use aether_data::{
    driver::{postgres::PostgresPool, mysql::MySqlPool},
    lifecycle::{backfill::PendingBackfillInfo, migrate::PendingMigrationInfo},
};
"#;
    let mut violations = Vec::new();
    collect_alias_violations(
        Path::new("crates/example/src/lib.rs"),
        source,
        "aether_data",
        &mut violations,
    );
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn grouped_import_scanner_rejects_top_level_old_paths() {
    let source = r#"
use aether_data::{
    postgres::PostgresPool,
    redis::{RedisKvRunner, RedisLockRunner},
};
"#;
    let mut violations = Vec::new();
    collect_alias_violations(
        Path::new("crates/example/src/lib.rs"),
        source,
        "aether_data",
        &mut violations,
    );
    assert_eq!(violations.len(), 2, "{violations:#?}");
}
