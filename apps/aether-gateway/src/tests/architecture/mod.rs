use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("directory should be readable") {
        let entry = entry.expect("directory entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

pub(super) fn assert_no_sqlx_queries(root_relative_path: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let patterns = [
        "sqlx::query(",
        "sqlx::query_scalar",
        "query_scalar::<",
        "QueryBuilder<",
    ];
    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("source file should be readable");
            let hits = patterns
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if hits.is_empty() {
                None
            } else {
                Some(format!("{} -> {}", path.display(), hits.join(", ")))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "disallowed SQL ownership violations:\n{}",
        violations.join("\n")
    );
}

pub(super) fn assert_no_sensitive_log_patterns(root_relative_path: &str, patterns: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("source file should be readable");
            let hits = patterns
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if hits.is_empty() {
                None
            } else {
                Some(format!("{} -> {}", path.display(), hits.join(", ")))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "disallowed sensitive logging patterns:\n{}",
        violations.join("\n")
    );
}

pub(super) fn assert_no_module_dependency_patterns(root_relative_path: &str, patterns: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("source file should be readable");
            let hits = patterns
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if hits.is_empty() {
                None
            } else {
                Some(format!("{} -> {}", path.display(), hits.join(", ")))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "disallowed module dependency patterns:\n{}",
        violations.join("\n")
    );
}

pub(super) fn workspace_file_exists(root_relative_path: &str) -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(root_relative_path)
        .exists()
}

pub(super) fn read_workspace_file(path: &str) -> String {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    fs::read_to_string(workspace_root.join(path)).expect("source file should be readable")
}

mod admin_observability;
mod admin_provider;
mod admin_shared;
mod admin_system;
mod ai_pipeline;
mod runtime_and_security;
mod sql_and_data;
mod usage;
