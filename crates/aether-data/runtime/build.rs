use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() {
    if let Err(err) = build_empty_database_snapshot() {
        panic!("failed to build empty database snapshot: {err}");
    }
}

fn build_empty_database_snapshot() -> Result<(), Box<dyn Error>> {
    let manifest = manifest_path();
    let source_root = manifest.parent().ok_or_else(|| {
        std::io::Error::other("bootstrap manifest should have a parent directory")
    })?;
    let output = out_dir()?.join("empty_database_snapshot.sql");

    println!("cargo:rerun-if-changed={}", manifest.display());

    let mut snapshot = Vec::new();
    let manifest_source = fs::read_to_string(&manifest)?;
    for part in manifest_source.lines().map(str::trim) {
        if part.is_empty() || part.starts_with('#') {
            continue;
        }

        let fragment = source_root.join(part);
        println!("cargo:rerun-if-changed={}", fragment.display());
        snapshot.extend_from_slice(&fs::read(&fragment)?);
    }

    fs::write(output, snapshot)?;
    Ok(())
}

fn manifest_path() -> PathBuf {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"))
        .join("schema/bootstrap/postgres/manifest.txt")
}

fn out_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(PathBuf::from(env::var("OUT_DIR").map_err(|_| {
        std::io::Error::other("OUT_DIR should be set")
    })?))
}
