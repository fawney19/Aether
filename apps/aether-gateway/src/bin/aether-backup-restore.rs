use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use aether_gateway::{
    restore_backup_json, BackupDecryptionKey, BackupRestoreLimits,
    DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES, DEFAULT_BACKUP_MAX_JSON_BYTES,
};
use clap::Parser;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const HISTORICAL_KEYS_ENV: &str = "AETHER_BACKUP_HISTORICAL_KEYS_JSON";
const MAX_SECRET_FILE_BYTES: usize = 1024 * 1024;
const MAX_KEY_FILES: usize = 16;
const MAX_V2_KEY_CANDIDATES: usize = 256;
const MAX_LEGACY_V1_KEY_CANDIDATES: usize = 16;
const AUTOMATIC_KEY_ENV_VARS: [(&str, bool); 3] = [
    ("AETHER_BACKUP_ENCRYPTION_KEY", false),
    ("AETHER_GATEWAY_DATA_ENCRYPTION_KEY", true),
    ("ENCRYPTION_KEY", true),
];

#[derive(Debug, Parser)]
#[command(
    name = "aether-backup-restore",
    about = "Decrypt and verify an Aether S3 backup into a local JSON file"
)]
struct Args {
    /// Local encrypted .json.zst.aes256gcm file.
    #[arg(long)]
    input: PathBuf,

    /// Complete canonical S3 object key used when the backup was encrypted.
    #[arg(long)]
    object_key: String,

    /// Destination for verified JSON. Existing files are rejected by default.
    #[arg(long)]
    output: PathBuf,

    /// Plaintext secret file; may be repeated. The secret itself is never accepted as an argument.
    #[arg(long = "key-file")]
    key_files: Vec<PathBuf>,

    /// Structured JSON keyring file. May also be set with AETHER_BACKUP_KEYRING_FILE.
    #[arg(long, env = "AETHER_BACKUP_KEYRING_FILE")]
    keyring_file: Option<PathBuf>,

    /// Replace an existing output file atomically.
    #[arg(long)]
    overwrite: bool,

    /// Maximum encrypted input size in MiB.
    #[arg(long, default_value_t = mib(DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES), value_parser = clap::value_parser!(u64).range(1..=4096))]
    max_encrypted_mib: u64,

    /// Maximum decompressed JSON size in MiB.
    #[arg(long, default_value_t = mib(DEFAULT_BACKUP_MAX_JSON_BYTES), value_parser = clap::value_parser!(u64).range(1..=8192))]
    max_json_mib: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyringDocument {
    version: u8,
    #[serde(default)]
    keys: Vec<KeyringSecret>,
    #[serde(default)]
    legacy_v1: Vec<KeyringSecret>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KeyringSecret {
    Direct(String),
    Named {
        #[serde(alias = "key")]
        secret: String,
    },
}

impl KeyringSecret {
    fn into_secret(self) -> String {
        match self {
            Self::Direct(secret) | Self::Named { secret } => secret,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Message(String),

    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("backup restore failed: {0}")]
    Restore(#[from] aether_gateway::BackupRestoreError),
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), CliError> {
    reject_output_aliases(&args)?;
    let limits = BackupRestoreLimits {
        max_encrypted_bytes: checked_mib(args.max_encrypted_mib)?,
        max_json_bytes: checked_mib(args.max_json_mib)?,
    };
    let encrypted = read_limited_file(&args.input, limits.max_encrypted_bytes, true, false)?;
    let candidates = load_key_candidates(&args)?;
    if candidates.is_empty() {
        return Err(CliError::Message(format!(
            "no backup keys configured; set AETHER_BACKUP_ENCRYPTION_KEY, AETHER_GATEWAY_DATA_ENCRYPTION_KEY, or ENCRYPTION_KEY, or use a protected key/keyring file ({HISTORICAL_KEYS_ENV} is also supported)"
        )));
    }

    let cipher_sha256 = format!("{:x}", Sha256::digest(&encrypted));
    let restored = restore_backup_json(&args.object_key, &encrypted, &candidates, limits)?;
    write_atomic_private(&args.output, restored.json_bytes(), args.overwrite)?;
    let restored_scope = restored.scope().as_str();

    let summary = serde_json::json!({
        "status": "verified_json_written",
        "object_key": args.object_key,
        "output": args.output.display().to_string(),
        "cipher_sha256": cipher_sha256,
        "envelope_version": restored.envelope_version,
        "key_id": restored.key_id,
        "export_version": restored.export_version,
        "exported_at": restored.exported_at,
        "scope": restored_scope,
        "database_applied": false,
    });
    println!(
        "{}",
        serde_json::to_string(&summary).map_err(|error| {
            CliError::Message(format!("could not serialize restore summary: {error}"))
        })?
    );
    Ok(())
}

fn reject_output_aliases(args: &Args) -> Result<(), CliError> {
    let Ok(output) = fs::canonicalize(&args.output) else {
        return Ok(());
    };
    let mut protected_inputs = Vec::with_capacity(args.key_files.len() + 2);
    protected_inputs.push(&args.input);
    protected_inputs.extend(args.key_files.iter());
    if let Some(keyring_file) = &args.keyring_file {
        protected_inputs.push(keyring_file);
    }
    for input in protected_inputs {
        if fs::canonicalize(input).is_ok_and(|canonical| canonical == output) {
            return Err(CliError::Message(format!(
                "output {} must not replace the encrypted input or a key file",
                args.output.display()
            )));
        }
    }
    Ok(())
}

const fn mib(bytes: usize) -> u64 {
    (bytes / (1024 * 1024)) as u64
}

fn checked_mib(value: u64) -> Result<usize, CliError> {
    value
        .checked_mul(1024 * 1024)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| CliError::Message("configured size limit is too large".to_string()))
}

fn load_key_candidates(args: &Args) -> Result<Vec<BackupDecryptionKey>, CliError> {
    if args.key_files.len() > MAX_KEY_FILES {
        return Err(CliError::Message(format!(
            "at most {MAX_KEY_FILES} --key-file values are allowed"
        )));
    }
    let mut values = Vec::<(String, bool)>::new();
    append_automatic_environment_keys(&mut values, |env_name| std::env::var(env_name).ok())?;
    if let Ok(value) = std::env::var(HISTORICAL_KEYS_ENV) {
        append_keyring_document(
            &mut values,
            parse_keyring(value.as_bytes(), HISTORICAL_KEYS_ENV)?,
        )?;
    }
    if let Some(path) = &args.keyring_file {
        let bytes = read_limited_file(path, MAX_SECRET_FILE_BYTES, true, true)?;
        append_keyring_document(
            &mut values,
            parse_keyring(&bytes, &path.display().to_string())?,
        )?;
    }
    for path in &args.key_files {
        let bytes = read_limited_file(path, MAX_SECRET_FILE_BYTES, true, true)?;
        let value = String::from_utf8(bytes).map_err(|_| {
            CliError::Message(format!("key file {} is not valid UTF-8", path.display()))
        })?;
        push_secret(&mut values, value, true)?;
    }

    let unique = deduplicate_and_validate_key_values(values)?;
    unique
        .into_iter()
        .map(|(secret, allow_v1)| {
            if allow_v1 {
                BackupDecryptionKey::historical(secret)
            } else {
                BackupDecryptionKey::v2_only(secret)
            }
            .map_err(CliError::from)
        })
        .collect()
}

fn append_automatic_environment_keys(
    values: &mut Vec<(String, bool)>,
    mut get_env: impl FnMut(&str) -> Option<String>,
) -> Result<(), CliError> {
    for (env_name, allow_v1) in AUTOMATIC_KEY_ENV_VARS {
        if let Some(value) = get_env(env_name) {
            push_secret(values, value, allow_v1)?;
        }
    }
    Ok(())
}

fn deduplicate_and_validate_key_values(
    values: Vec<(String, bool)>,
) -> Result<Vec<(String, bool)>, CliError> {
    let mut indexes = HashMap::<String, usize>::new();
    let mut unique = Vec::<(String, bool)>::new();
    let mut legacy_v1_count = 0_usize;
    for (secret, allow_v1) in values {
        if let Some(index) = indexes.get(&secret).copied() {
            if allow_v1 && !unique[index].1 {
                unique[index].1 = true;
                legacy_v1_count += 1;
            }
        } else {
            let index = unique.len();
            indexes.insert(secret.clone(), index);
            unique.push((secret, allow_v1));
            legacy_v1_count += usize::from(allow_v1);

            if unique.len() > MAX_V2_KEY_CANDIDATES {
                return Err(CliError::Message(format!(
                    "backup restore allows at most {MAX_V2_KEY_CANDIDATES} v2 candidate keys"
                )));
            }
        }

        if legacy_v1_count > MAX_LEGACY_V1_KEY_CANDIDATES {
            return Err(CliError::Message(format!(
                "legacy v1 backup restore allows at most {MAX_LEGACY_V1_KEY_CANDIDATES} candidate keys"
            )));
        }
    }
    Ok(unique)
}

fn parse_keyring(bytes: &[u8], source: &str) -> Result<KeyringDocument, CliError> {
    serde_json::from_slice(bytes).map_err(|error| {
        CliError::Message(format!(
            "keyring {source} is not valid structured JSON: {error}"
        ))
    })
}

fn append_keyring_document(
    values: &mut Vec<(String, bool)>,
    keyring: KeyringDocument,
) -> Result<(), CliError> {
    if keyring.version != 1 {
        return Err(CliError::Message(format!(
            "unsupported backup keyring version {}",
            keyring.version
        )));
    }
    for secret in keyring.keys {
        push_secret(values, secret.into_secret(), false)?;
    }
    for secret in keyring.legacy_v1 {
        push_secret(values, secret.into_secret(), true)?;
    }
    Ok(())
}

fn push_secret(
    values: &mut Vec<(String, bool)>,
    value: String,
    allow_v1: bool,
) -> Result<(), CliError> {
    let value = value.trim().to_string();
    if value.is_empty() || value.contains('\0') {
        return Err(CliError::Message(
            "backup key material must not be empty or contain NUL".to_string(),
        ));
    }
    values.push((value, allow_v1));
    Ok(())
}

fn read_limited_file(
    path: &Path,
    limit: usize,
    reject_symlink: bool,
    require_private_permissions: bool,
) -> Result<Vec<u8>, CliError> {
    let mut file = open_file_without_following_symlinks(path, reject_symlink)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(CliError::Message(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if require_private_permissions {
        validate_secret_file_permissions(path, &metadata)?;
    }
    #[cfg(not(unix))]
    if reject_symlink && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(CliError::Message(format!(
            "input file {} changed to a symbolic link while being opened",
            path.display()
        )));
    }
    if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(CliError::Message(format!(
            "{} exceeds the configured {} byte limit",
            path.display(),
            limit
        )));
    }
    let read_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(limit));
    Read::by_ref(&mut file)
        .take(read_limit)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(CliError::Message(format!(
            "{} exceeds the configured {} byte limit",
            path.display(),
            limit
        )));
    }
    Ok(bytes)
}

fn open_file_without_following_symlinks(
    path: &Path,
    reject_symlink: bool,
) -> Result<File, CliError> {
    #[cfg(unix)]
    if reject_symlink {
        return open_file_beneath_real_directories(path);
    }

    #[cfg(not(unix))]
    if reject_symlink && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(CliError::Message(format!(
            "input file {} must not be a symbolic link",
            path.display()
        )));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    if reject_symlink {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(CliError::from)
}

#[cfg(unix)]
fn open_file_beneath_real_directories(path: &Path) -> Result<File, CliError> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let (parent, file_name) = open_real_parent_directory(path)?;
    let file_name = unix_path_component(&file_name, "input file name")?;
    // SAFETY: `parent` is a valid open directory descriptor, `file_name` is NUL-terminated,
    // and ownership of a successful descriptor is transferred immediately to `File`.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            file_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::ELOOP) | Some(libc::ENOTDIR)
        ) && unix_file_mode_at(&parent, &file_name)?
            .is_some_and(|mode| mode & libc::S_IFMT == libc::S_IFLNK)
        {
            return Err(CliError::Message(format!(
                "input file {} must not be a symbolic link",
                path.display()
            )));
        }
        return Err(CliError::Io(error));
    }
    // SAFETY: `openat` returned a new owned descriptor and no other owner exists.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn open_real_parent_directory(path: &Path) -> Result<(File, OsString), CliError> {
    let file_name = path.file_name().map(OsString::from).ok_or_else(|| {
        CliError::Message(format!(
            "path {} must include a regular file name",
            path.display()
        ))
    })?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok((open_real_directory(parent)?, file_name))
}

#[cfg(unix)]
fn open_real_directory(path: &Path) -> Result<File, CliError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::path::Component;

    let mut directory = File::open(if path.is_absolute() { "/" } else { "." })?;
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir => {
                return Err(CliError::Message(format!(
                    "path {} must not contain '..' components",
                    path.display()
                )))
            }
            Component::Prefix(_) => {
                return Err(CliError::Message(format!(
                    "path {} uses an unsupported prefix",
                    path.display()
                )))
            }
        };
        let name = unix_path_component(name, "directory component")?;
        // SAFETY: `directory` is a valid open directory descriptor and `name` is a valid
        // NUL-terminated component. A successful descriptor is immediately owned by `File`.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::ELOOP) | Some(libc::ENOTDIR)
            ) && unix_file_mode_at(&directory, &name)?
                .is_some_and(|mode| mode & libc::S_IFMT == libc::S_IFLNK)
            {
                return Err(CliError::Message(format!(
                    "path {} must not contain symbolic-link directory components",
                    path.display()
                )));
            }
            return Err(CliError::Io(error));
        }
        // SAFETY: `openat` returned a new owned descriptor and no other owner exists.
        directory = unsafe { File::from_raw_fd(descriptor) };
    }
    Ok(directory)
}

#[cfg(unix)]
fn unix_path_component(
    component: &std::ffi::OsStr,
    description: &str,
) -> Result<std::ffi::CString, CliError> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(component.as_bytes()).map_err(|_| {
        CliError::Message(format!(
            "{description} must not contain an embedded NUL byte"
        ))
    })
}

#[cfg(unix)]
fn validate_secret_file_permissions(path: &Path, metadata: &fs::Metadata) -> Result<(), CliError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CliError::Message(format!(
            "secret file {} is accessible by group or other users; require mode 0600 or stricter",
            path.display()
        )));
    }
    // SAFETY: `geteuid` has no preconditions and does not retain pointers or borrowed state.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(CliError::Message(format!(
            "secret file {} must be owned by the current effective user (uid {effective_uid})",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_secret_file_permissions(
    _path: &Path,
    _metadata: &fs::Metadata,
) -> Result<(), CliError> {
    Ok(())
}

#[cfg(unix)]
fn write_atomic_private(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), CliError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::PermissionsExt;

    let (parent, output_file_name) = open_real_parent_directory(path)?;
    let output_name = unix_path_component(&output_file_name, "output file name")?;
    if let Some(mode) = unix_file_mode_at(&parent, &output_name)? {
        if mode & libc::S_IFMT != libc::S_IFREG {
            return Err(CliError::Message(format!(
                "output {} must be a regular file path, not a symbolic link or special file",
                path.display()
            )));
        }
        if !overwrite {
            return Err(CliError::Message(format!(
                "output {} already exists; pass --overwrite to replace it",
                path.display()
            )));
        }
    }

    let safe_file_name = safe_temp_file_component(&output_file_name);
    let temp_file_name = OsString::from(format!(
        ".{}.aether-restore-{}-{}.tmp",
        safe_file_name.to_string_lossy(),
        std::process::id(),
        Uuid::new_v4()
    ));
    let temp_name = unix_path_component(&temp_file_name, "temporary output file name")?;
    // SAFETY: `parent` is a valid directory descriptor, `temp_name` is NUL-terminated, and a
    // successful descriptor is transferred immediately to `File`. O_EXCL prevents name reuse.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            temp_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(CliError::Io(std::io::Error::last_os_error()));
    }
    // SAFETY: `openat` returned a new owned descriptor and no other owner exists.
    let mut temp = unsafe { File::from_raw_fd(descriptor) };

    let result = (|| -> Result<(), CliError> {
        temp.set_permissions(fs::Permissions::from_mode(0o600))?;
        temp.write_all(bytes)?;
        temp.sync_all()?;
        drop(temp);
        if overwrite {
            unix_rename_at(&parent, &temp_name, &output_name)?;
        } else {
            unix_link_at(&parent, &temp_name, &output_name).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    CliError::Message(format!(
                        "output {} already exists; pass --overwrite to replace it",
                        path.display()
                    ))
                } else {
                    CliError::Io(error)
                }
            })?;
            unix_unlink_at(&parent, &temp_name)?;
        }
        parent.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = unix_unlink_at(&parent, &temp_name);
    }
    result
}

#[cfg(unix)]
fn unix_file_mode_at(
    parent: &File,
    file_name: &std::ffi::CStr,
) -> Result<Option<libc::mode_t>, CliError> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;

    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `parent` and `file_name` remain valid for the call, and `stat` points to writable
    // storage. The value is initialized only when fstatat reports success.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            file_name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        // SAFETY: successful fstatat initialized the complete stat structure.
        return Ok(Some(unsafe { stat.assume_init() }.st_mode));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(None)
    } else {
        Err(CliError::Io(error))
    }
}

#[cfg(unix)]
fn unix_link_at(
    parent: &File,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> Result<(), std::io::Error> {
    use std::os::fd::AsRawFd;

    // SAFETY: both names are valid NUL-terminated components and both directory descriptors are
    // the same live `parent` descriptor. No pointers are retained after the call.
    let result = unsafe {
        libc::linkat(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unix_rename_at(
    parent: &File,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> Result<(), CliError> {
    use std::os::fd::AsRawFd;

    // SAFETY: both names are valid NUL-terminated components and `parent` remains open for the
    // duration of the call. renameat does not retain either pointer.
    let result = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(CliError::Io(std::io::Error::last_os_error()))
    }
}

#[cfg(unix)]
fn unix_unlink_at(parent: &File, file_name: &std::ffi::CStr) -> Result<(), CliError> {
    use std::os::fd::AsRawFd;

    // SAFETY: `parent` is a valid directory descriptor and `file_name` is a NUL-terminated
    // component. unlinkat does not retain either argument.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), file_name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(CliError::Io(std::io::Error::last_os_error()))
    }
}

#[cfg(not(unix))]
fn write_atomic_private(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), CliError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name =
        safe_temp_file_component(path.file_name().ok_or_else(|| {
            CliError::Message("output path must include a file name".to_string())
        })?);
    if !parent.is_dir() {
        return Err(CliError::Message(format!(
            "output directory {} does not exist",
            parent.display()
        )));
    }
    if fs::symlink_metadata(parent)?.file_type().is_symlink() {
        return Err(CliError::Message(format!(
            "output directory {} must not be a symbolic link",
            parent.display()
        )));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || metadata.is_dir() || !metadata.is_file() {
            return Err(CliError::Message(format!(
                "output {} must be a regular file path, not a symbolic link or special file",
                path.display()
            )));
        }
    }
    if !overwrite && fs::symlink_metadata(path).is_ok() {
        return Err(CliError::Message(format!(
            "output {} already exists; pass --overwrite to replace it",
            path.display()
        )));
    }

    let temp_name = format!(
        ".{}.aether-restore-{}-{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        Uuid::new_v4()
    );
    let temp_path = parent.join(temp_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut temp = options.open(&temp_path)?;
    let result = (|| -> Result<(), CliError> {
        temp.write_all(bytes)?;
        temp.sync_all()?;
        drop(temp);
        if overwrite {
            replace_output_file(&temp_path, path)?;
        } else {
            fs::hard_link(&temp_path, path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    CliError::Message(format!(
                        "output {} already exists; pass --overwrite to replace it",
                        path.display()
                    ))
                } else {
                    CliError::Io(error)
                }
            })?;
            fs::remove_file(&temp_path)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn safe_temp_file_component(file_name: &std::ffi::OsStr) -> OsString {
    let sanitized: String = file_name
        .to_string_lossy()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character => character,
        })
        .collect();
    OsString::from(sanitized)
}

#[cfg(test)]
mod tests {
    use super::{
        append_automatic_environment_keys, deduplicate_and_validate_key_values, read_limited_file,
        write_atomic_private, MAX_LEGACY_V1_KEY_CANDIDATES, MAX_V2_KEY_CANDIDATES,
    };

    #[cfg(unix)]
    fn unix_test_directory(prefix: &str) -> std::path::PathBuf {
        std::fs::canonicalize(std::env::temp_dir())
            .expect("system temporary directory should canonicalize")
            .join(format!("{prefix}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn automatic_key_environment_priority_matches_backup_generation() {
        let mut requested = Vec::new();
        let mut values = Vec::new();
        append_automatic_environment_keys(&mut values, |env_name| {
            requested.push(env_name.to_string());
            Some(format!("secret-for-{env_name}"))
        })
        .expect("automatic keys should load");

        assert_eq!(
            requested,
            vec![
                "AETHER_BACKUP_ENCRYPTION_KEY",
                "AETHER_GATEWAY_DATA_ENCRYPTION_KEY",
                "ENCRYPTION_KEY",
            ]
        );
        assert_eq!(
            values,
            vec![
                ("secret-for-AETHER_BACKUP_ENCRYPTION_KEY".to_string(), false,),
                (
                    "secret-for-AETHER_GATEWAY_DATA_ENCRYPTION_KEY".to_string(),
                    true,
                ),
                ("secret-for-ENCRYPTION_KEY".to_string(), true),
            ]
        );
    }

    #[test]
    fn candidate_deduplication_preserves_priority_and_promotes_legacy_access() {
        let candidates = deduplicate_and_validate_key_values(vec![
            ("backup-current".to_string(), false),
            ("gateway-fallback".to_string(), true),
            ("default-fallback".to_string(), true),
            ("backup-current".to_string(), true),
        ])
        .expect("candidate set should be valid");

        assert_eq!(
            candidates,
            vec![
                ("backup-current".to_string(), true),
                ("gateway-fallback".to_string(), true),
                ("default-fallback".to_string(), true),
            ]
        );
    }

    #[test]
    fn rejects_too_many_v2_candidates_before_key_construction() {
        let values = (0..=MAX_V2_KEY_CANDIDATES)
            .map(|index| (format!("v2-key-{index}"), false))
            .collect();

        let error = deduplicate_and_validate_key_values(values)
            .expect_err("candidate count above the restore limit must fail");

        assert_eq!(
            error.to_string(),
            format!("backup restore allows at most {MAX_V2_KEY_CANDIDATES} v2 candidate keys")
        );
    }

    #[test]
    fn accepts_candidate_counts_at_both_restore_limits() {
        let values = (0..MAX_LEGACY_V1_KEY_CANDIDATES)
            .map(|index| (format!("legacy-key-{index}"), true))
            .chain(
                (MAX_LEGACY_V1_KEY_CANDIDATES..MAX_V2_KEY_CANDIDATES)
                    .map(|index| (format!("v2-key-{index}"), false)),
            )
            .collect();

        let candidates = deduplicate_and_validate_key_values(values)
            .expect("candidate counts at the restore limits must be accepted");

        assert_eq!(candidates.len(), MAX_V2_KEY_CANDIDATES);
        assert_eq!(
            candidates.iter().filter(|(_, allow_v1)| *allow_v1).count(),
            MAX_LEGACY_V1_KEY_CANDIDATES
        );
    }

    #[test]
    fn rejects_too_many_legacy_candidates_before_key_construction() {
        let values = (0..=MAX_LEGACY_V1_KEY_CANDIDATES)
            .map(|index| (format!("legacy-key-{index}"), true))
            .collect();

        let error = deduplicate_and_validate_key_values(values)
            .expect_err("legacy candidate count above the restore limit must fail");

        assert_eq!(
            error.to_string(),
            format!(
                "legacy v1 backup restore allows at most {MAX_LEGACY_V1_KEY_CANDIDATES} candidate keys"
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn encrypted_input_symbolic_links_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = unix_test_directory("aether-backup-restore-symlink-test");
        std::fs::create_dir(&directory).expect("test directory should be created");
        let target = directory.join("backup.bin");
        let link = directory.join("backup-link.bin");
        std::fs::write(&target, b"encrypted-backup").expect("test target should be written");
        symlink(&target, &link).expect("test symlink should be created");

        let error = read_limited_file(&link, 1024, true, false)
            .expect_err("encrypted backup symlink must be rejected");
        assert!(error.to_string().contains("must not be a symbolic link"));

        std::fs::remove_dir_all(directory).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn encrypted_input_symbolic_link_ancestors_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = unix_test_directory("aether-backup-restore-ancestor-test");
        let real_parent = directory.join("real-parent");
        let linked_parent = directory.join("linked-parent");
        std::fs::create_dir_all(&real_parent).expect("real parent should be created");
        std::fs::write(real_parent.join("backup.bin"), b"encrypted-backup")
            .expect("test input should be written");
        symlink(&real_parent, &linked_parent).expect("parent symlink should be created");

        read_limited_file(&linked_parent.join("backup.bin"), 1024, true, false)
            .expect_err("a symbolic-link ancestor must be rejected");

        std::fs::remove_dir_all(directory).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_private_output_rejects_symbolic_link_ancestors() {
        use std::os::unix::fs::symlink;

        let directory = unix_test_directory("aether-backup-output-ancestor-test");
        let real_parent = directory.join("real-parent");
        let linked_parent = directory.join("linked-parent");
        std::fs::create_dir_all(&real_parent).expect("real parent should be created");
        symlink(&real_parent, &linked_parent).expect("parent symlink should be created");

        write_atomic_private(&linked_parent.join("restored.json"), b"{}", false)
            .expect_err("output through a symbolic-link ancestor must be rejected");
        assert!(!real_parent.join("restored.json").exists());

        std::fs::remove_dir_all(directory).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_private_output_is_mode_0600_and_preserves_no_overwrite() {
        use std::os::unix::fs::PermissionsExt;

        let directory = unix_test_directory("aether-backup-output-mode-test");
        std::fs::create_dir(&directory).expect("test directory should be created");
        let output = directory.join("restored.json");

        write_atomic_private(&output, b"first", false).expect("first output should be written");
        assert_eq!(
            std::fs::metadata(&output)
                .expect("output metadata should load")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        write_atomic_private(&output, b"second", false)
            .expect_err("no-overwrite mode must preserve an existing output");
        assert_eq!(
            std::fs::read(&output).expect("output should remain readable"),
            b"first"
        );

        write_atomic_private(&output, b"second", true)
            .expect("overwrite mode should atomically replace a regular output");
        assert_eq!(
            std::fs::read(&output).expect("replaced output should be readable"),
            b"second"
        );

        std::fs::remove_dir_all(directory).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_require_current_owner_and_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = unix_test_directory("aether-backup-secret-permissions-test");
        std::fs::create_dir(&directory).expect("test directory should be created");
        let secret_file = directory.join("backup.key");
        std::fs::write(&secret_file, b"private-backup-key").expect("test secret should be written");
        std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o600))
            .expect("test secret permissions should be private");

        assert_eq!(
            read_limited_file(&secret_file, 1024, true, true)
                .expect("current-user-owned private secret should load"),
            b"private-backup-key"
        );

        std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o640))
            .expect("test secret permissions should become group-readable");
        assert!(read_limited_file(&secret_file, 1024, true, true)
            .expect_err("group-readable secret must be rejected")
            .to_string()
            .contains("accessible by group or other users"));

        std::fs::remove_dir_all(directory).expect("test directory should be removed");
    }
}

#[cfg(windows)]
fn replace_output_file(temp_path: &Path, path: &Path) -> Result<(), CliError> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(CliError::Message(
            "--overwrite cannot atomically replace an existing file on Windows; choose a new output path"
                .to_string(),
        ));
    }
    fs::rename(temp_path, path).map_err(CliError::from)
}

#[cfg(not(any(unix, windows)))]
fn replace_output_file(temp_path: &Path, path: &Path) -> Result<(), CliError> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(CliError::Message(
            "--overwrite is unsupported on this platform; choose a new output path".to_string(),
        ));
    }
    fs::rename(temp_path, path).map_err(CliError::from)
}
