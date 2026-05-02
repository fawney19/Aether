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

pub(super) fn collect_workspace_rust_files(root_relative_path: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);
    files.sort();
    files
}

pub(super) fn read_workspace_file(path: &str) -> String {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    fs::read_to_string(workspace_root.join(path)).expect("source file should be readable")
}

pub(super) fn read_workspace_module_tree(path: &str) -> String {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let root_path = workspace_root.join(path);
    let mut contents =
        vec![fs::read_to_string(&root_path).expect("source file should be readable")];

    let module_dir = if root_path.file_name().and_then(|value| value.to_str()) == Some("mod.rs") {
        root_path
            .parent()
            .expect("mod.rs should have parent module directory")
            .to_path_buf()
    } else if root_path.extension().and_then(|value| value.to_str()) == Some("rs") {
        root_path.with_extension("")
    } else {
        root_path.clone()
    };
    if module_dir.is_dir() {
        let mut files = Vec::new();
        collect_rust_files(&module_dir, &mut files);
        files.sort();
        for file in files {
            contents.push(fs::read_to_string(file).expect("source file should be readable"));
        }
    }

    contents.join("\n")
}

mod admin_billing;
mod admin_model;
mod admin_observability;
mod admin_provider;
mod admin_shared;
mod admin_system;
mod admin_users;
mod ai_serving;
mod runtime_and_security;
mod sql_and_data;
mod usage;
