use std::cmp::Reverse;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local, TimeZone};
use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;

use crate::config::ServiceRuntimeConfig;
use crate::error::RuntimeBootstrapError;
use crate::observability::{FileLoggingConfig, LogDestination, LogRotation};

static TRACING_INIT: OnceLock<Result<(), String>> = OnceLock::new();

pub type LogReloader = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone)]
struct RuntimeLogIdentity {
    service: &'static str,
    node_role: Option<String>,
    instance_id: Option<String>,
}

impl RuntimeLogIdentity {
    fn from_config(config: &ServiceRuntimeConfig) -> Self {
        Self {
            service: config.service_name,
            node_role: config.observability.node_role.clone(),
            instance_id: config.observability.instance_id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct StartupCleanupWarning {
    log_dir: PathBuf,
    error: String,
}

#[derive(Debug, Default)]
struct RuntimeFieldVisitor {
    fields: Vec<(String, Value)>,
}

impl RuntimeFieldVisitor {
    fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    fn take_message(&mut self) -> Option<String> {
        if let Some(pos) = self.fields.iter().position(|(k, _)| k == "message") {
            let (_, value) = self.fields.remove(pos);
            match value {
                Value::String(s) => Some(s),
                other => Some(other.to_string()),
            }
        } else {
            None
        }
    }

    fn into_json_object(self) -> Map<String, Value> {
        self.fields.into_iter().collect()
    }

    fn record_value(&mut self, field: &Field, value: Value) {
        if let Some((_, existing)) = self
            .fields
            .iter_mut()
            .find(|(name, _)| name.as_str() == field.name())
        {
            *existing = value;
            return;
        }
        self.fields.push((field.name().to_string(), value));
    }

    fn write_pretty(&self, writer: &mut Writer<'_>, ansi: bool) -> fmt::Result {
        for (index, (key, value)) in self.fields.iter().enumerate() {
            if index > 0 {
                writer.write_char(' ')?;
            }
            write_colored(writer, key, ansi.then_some(ANSI_DIM))?;
            writer.write_char('=')?;
            write_pretty_value(writer, value)?;
        }
        Ok(())
    }
}

impl Visit for RuntimeFieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field, Value::Bool(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_value(field, serde_json::json!(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field, Value::from(value));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.record_value(field, Value::String(value.to_string()));
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.record_value(field, Value::String(value.to_string()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field, Value::String(value.to_string()));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_value(field, Value::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record_value(field, Value::String(format!("{value:?}")));
    }
}

#[derive(Debug, Clone)]
struct PrettyRuntimeEventFormatter {
    _identity: RuntimeLogIdentity,
    ansi: bool,
}

impl PrettyRuntimeEventFormatter {
    fn new(identity: RuntimeLogIdentity, ansi: bool) -> Self {
        Self {
            _identity: identity,
            ansi,
        }
    }
}

impl<S, N> FormatEvent<S, N> for PrettyRuntimeEventFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let mut fields = RuntimeFieldVisitor::default();
        event.record(&mut fields);

        let level_color = self.ansi.then_some(level_ansi(meta.level()));
        let message = fields.take_message();
        let depth = ctx.event_scope().map(|scope| scope.count()).unwrap_or(0);

        // timestamp (green)
        write_colored(
            &mut writer,
            &formatted_timestamp(),
            self.ansi.then_some(ANSI_GREEN),
        )?;
        // separator
        write_separator(&mut writer, self.ansi)?;
        // level (bold + level color, padded to 8 chars like loguru)
        write_colored(&mut writer, &format!("{:<8}", meta.level()), level_color)?;
        // separator
        write_separator(&mut writer, self.ansi)?;
        // target (cyan, shortened/truncated to fixed width)
        let target_cell = format_target_cell(meta.target(), TARGET_COLUMN_WIDTH);
        write_colored(&mut writer, &target_cell, self.ansi.then_some(ANSI_CYAN))?;
        // message (level color, after " - ")
        if let Some(ref msg) = message {
            write_colored(&mut writer, " - ", self.ansi.then_some(ANSI_DIM))?;
            let prefix = span_tree_prefix(depth);
            if !prefix.is_empty() {
                write_colored(&mut writer, &prefix, self.ansi.then_some(ANSI_DIM))?;
            }
            write_colored(&mut writer, msg, level_color)?;
        }
        // remaining structured fields
        if !fields.is_empty() {
            write_separator(&mut writer, self.ansi)?;
            fields.write_pretty(&mut writer, self.ansi)?;
        }
        writeln!(writer)
    }
}

#[derive(Debug, Clone)]
struct JsonRuntimeEventFormatter {
    identity: RuntimeLogIdentity,
}

impl JsonRuntimeEventFormatter {
    fn new(identity: RuntimeLogIdentity) -> Self {
        Self { identity }
    }
}

impl<S, N> FormatEvent<S, N> for JsonRuntimeEventFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let mut fields = RuntimeFieldVisitor::default();
        event.record(&mut fields);
        let depth = ctx.event_scope().map(|scope| scope.count()).unwrap_or(0);

        let mut payload = Map::new();
        payload.insert(
            "timestamp".to_string(),
            Value::String(formatted_timestamp()),
        );
        payload.insert("level".to_string(), Value::String(meta.level().to_string()));
        payload.insert(
            "service".to_string(),
            Value::String(self.identity.service.to_string()),
        );
        payload.insert(
            "node_role".to_string(),
            self.identity
                .node_role
                .as_ref()
                .map(|value| Value::String(value.clone()))
                .unwrap_or(Value::Null),
        );
        payload.insert(
            "instance_id".to_string(),
            self.identity
                .instance_id
                .as_ref()
                .map(|value| Value::String(value.clone()))
                .unwrap_or(Value::Null),
        );
        payload.insert(
            "target".to_string(),
            Value::String(meta.target().to_string()),
        );
        payload.insert("span_depth".to_string(), Value::from(depth as u64));
        payload.insert(
            "fields".to_string(),
            Value::Object(fields.into_json_object()),
        );

        writer
            .write_str(&serde_json::to_string(&Value::Object(payload)).map_err(|_| fmt::Error)?)?;
        writeln!(writer)
    }
}

fn formatted_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string()
}

const TARGET_COLUMN_WIDTH: usize = 24;

fn shorten_target(target: &str, max_segments: usize) -> &str {
    let mut count = 0usize;
    for (idx, _) in target.rmatch_indices("::") {
        count += 1;
        if count >= max_segments {
            return &target[idx + 2..];
        }
    }
    target
}

fn span_tree_prefix(depth: usize) -> String {
    if depth == 0 {
        return String::new();
    }

    let mut prefix = String::new();
    for _ in 0..depth.saturating_sub(1) {
        prefix.push_str("│  ");
    }
    prefix.push_str("├─ ");
    prefix
}

fn format_target_cell(target: &str, width: usize) -> String {
    let short_target = shorten_target(target, 2);
    let len = short_target.chars().count();
    if len <= width {
        let padding = " ".repeat(width - len);
        return format!("{short_target}{padding}");
    }

    let tail: String = short_target
        .chars()
        .rev()
        .take(width.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("~{tail}")
}

const ANSI_RESET: &str = "\u{1b}[0m";
const ANSI_DIM: &str = "\u{1b}[2m";
const ANSI_BOLD: &str = "\u{1b}[1m";
const ANSI_GREEN: &str = "\u{1b}[32m";
const ANSI_CYAN: &str = "\u{1b}[36m";
const ANSI_BOLD_RED: &str = "\u{1b}[1;31m";
const ANSI_BOLD_YELLOW: &str = "\u{1b}[1;33m";
const ANSI_BOLD_BLUE: &str = "\u{1b}[1;34m";
const ANSI_BOLD_MAGENTA: &str = "\u{1b}[1;35m";

fn write_colored(writer: &mut Writer<'_>, value: &str, ansi_code: Option<&str>) -> fmt::Result {
    if let Some(code) = ansi_code {
        writer.write_str(code)?;
        writer.write_str(value)?;
        writer.write_str(ANSI_RESET)
    } else {
        writer.write_str(value)
    }
}

fn write_separator(writer: &mut Writer<'_>, ansi: bool) -> fmt::Result {
    write_colored(writer, " | ", ansi.then_some(ANSI_DIM))
}

fn level_ansi(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::ERROR => ANSI_BOLD_RED,
        tracing::Level::WARN => ANSI_BOLD_YELLOW,
        tracing::Level::INFO => ANSI_BOLD,
        tracing::Level::DEBUG => ANSI_BOLD_BLUE,
        tracing::Level::TRACE => ANSI_BOLD_MAGENTA,
    }
}

fn stdout_supports_ansi() -> bool {
    io::stdout().is_terminal()
}

fn write_pretty_value(writer: &mut Writer<'_>, value: &Value) -> fmt::Result {
    match value {
        Value::String(text) => write!(writer, "{text:?}"),
        Value::Number(number) => write!(writer, "{number}"),
        Value::Bool(boolean) => write!(writer, "{boolean}"),
        Value::Null => writer.write_str("null"),
        Value::Array(_) | Value::Object(_) => {
            writer.write_str(&serde_json::to_string(value).map_err(|_| fmt::Error)?)
        }
    }
}

pub(crate) fn init_tracing(config: ServiceRuntimeConfig) -> Result<(), RuntimeBootstrapError> {
    TRACING_INIT
        .get_or_init(|| {
            let filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| config.default_log_filter.into());
            let identity = RuntimeLogIdentity::from_config(&config);

            let (file_writer, startup_cleanup_warning) =
                if config.observability.log_destination.needs_file_sink() {
                    let Some(file_logging) = config.observability.file_logging.clone() else {
                        return Err("file logging requires a configured log directory".to_string());
                    };
                    let (writer, startup_cleanup_warning) =
                        RollingFileMakeWriter::new(config.service_name, file_logging)?;
                    (Some(writer), startup_cleanup_warning)
                } else {
                    (None, None)
                };

            let init_result = match (
                config.observability.log_format,
                config.observability.log_destination,
            ) {
                (LogFormat::Pretty, LogDestination::Stdout) => tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().event_format(
                        PrettyRuntimeEventFormatter::new(identity.clone(), stdout_supports_ansi()),
                    ))
                    .try_init(),
                (LogFormat::Json, LogDestination::Stdout) => tracing_subscriber::registry()
                    .with(filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .event_format(JsonRuntimeEventFormatter::new(identity.clone())),
                    )
                    .try_init(),
                (LogFormat::Pretty, LogDestination::File) => tracing_subscriber::registry()
                    .with(filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(false)
                            .event_format(PrettyRuntimeEventFormatter::new(identity.clone(), false))
                            .with_writer(file_writer.clone().expect("file writer should exist")),
                    )
                    .try_init(),
                (LogFormat::Json, LogDestination::File) => tracing_subscriber::registry()
                    .with(filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .event_format(JsonRuntimeEventFormatter::new(identity.clone()))
                            .with_writer(file_writer.clone().expect("file writer should exist")),
                    )
                    .try_init(),
                (LogFormat::Pretty, LogDestination::Both) => tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().event_format(
                        PrettyRuntimeEventFormatter::new(identity.clone(), stdout_supports_ansi()),
                    ))
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(false)
                            .event_format(PrettyRuntimeEventFormatter::new(identity.clone(), false))
                            .with_writer(file_writer.clone().expect("file writer should exist")),
                    )
                    .try_init(),
                (LogFormat::Json, LogDestination::Both) => tracing_subscriber::registry()
                    .with(filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .event_format(JsonRuntimeEventFormatter::new(identity.clone())),
                    )
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .event_format(JsonRuntimeEventFormatter::new(identity.clone()))
                            .with_writer(file_writer.clone().expect("file writer should exist")),
                    )
                    .try_init(),
            }
            .map_err(|err| err.to_string());

            if init_result.is_ok() {
                if let Some(warning) = startup_cleanup_warning.as_ref() {
                    emit_log_cleanup_warning("startup", warning.log_dir.as_path(), &warning.error);
                }
                if let Some(file_logging) = config.observability.file_logging.clone() {
                    spawn_log_cleanup_task(config.service_name, file_logging);
                }
            }

            init_result
        })
        .clone()
        .map_err(RuntimeBootstrapError::Tracing)
}

pub fn init_reloadable_tracing(
    initial_filter: &str,
    format: LogFormat,
) -> Result<LogReloader, RuntimeBootstrapError> {
    init_reloadable_service_tracing(
        initial_filter,
        ServiceRuntimeConfig::new("runtime", "info").with_log_format(format),
    )
}

pub fn init_reloadable_service_tracing(
    initial_filter: &str,
    config: ServiceRuntimeConfig,
) -> Result<LogReloader, RuntimeBootstrapError> {
    use tracing_subscriber::reload;

    let filter = EnvFilter::try_new(initial_filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter_layer, reload_handle) = reload::Layer::new(filter);
    let identity = RuntimeLogIdentity::from_config(&config);

    let (file_writer, startup_cleanup_warning) =
        if config.observability.log_destination.needs_file_sink() {
            let Some(file_logging) = config.observability.file_logging.clone() else {
                return Err(RuntimeBootstrapError::Tracing(
                    "file logging requires a configured log directory".to_string(),
                ));
            };
            let (writer, startup_cleanup_warning) =
                RollingFileMakeWriter::new(config.service_name, file_logging)
                    .map_err(RuntimeBootstrapError::Tracing)?;
            (Some(writer), startup_cleanup_warning)
        } else {
            (None, None)
        };

    match (
        config.observability.log_format,
        config.observability.log_destination,
    ) {
        (LogFormat::Pretty, LogDestination::Stdout) => {
            tracing_subscriber::registry()
                .with(filter_layer)
                .with(tracing_subscriber::fmt::layer().event_format(
                    PrettyRuntimeEventFormatter::new(identity.clone(), stdout_supports_ansi()),
                ))
                .try_init()
        }
        (LogFormat::Json, LogDestination::Stdout) => tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .event_format(JsonRuntimeEventFormatter::new(identity.clone())),
            )
            .try_init(),
        (LogFormat::Pretty, LogDestination::File) => tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .event_format(PrettyRuntimeEventFormatter::new(identity.clone(), false))
                    .with_writer(file_writer.clone().expect("file writer should exist")),
            )
            .try_init(),
        (LogFormat::Json, LogDestination::File) => tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .event_format(JsonRuntimeEventFormatter::new(identity.clone()))
                    .with_writer(file_writer.clone().expect("file writer should exist")),
            )
            .try_init(),
        (LogFormat::Pretty, LogDestination::Both) => {
            tracing_subscriber::registry()
                .with(filter_layer)
                .with(tracing_subscriber::fmt::layer().event_format(
                    PrettyRuntimeEventFormatter::new(identity.clone(), stdout_supports_ansi()),
                ))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .event_format(PrettyRuntimeEventFormatter::new(identity.clone(), false))
                        .with_writer(file_writer.clone().expect("file writer should exist")),
                )
                .try_init()
        }
        (LogFormat::Json, LogDestination::Both) => tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .event_format(JsonRuntimeEventFormatter::new(identity.clone())),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .event_format(JsonRuntimeEventFormatter::new(identity.clone()))
                    .with_writer(file_writer.clone().expect("file writer should exist")),
            )
            .try_init(),
    }
    .map_err(|err| RuntimeBootstrapError::Tracing(err.to_string()))?;

    if let Some(warning) = startup_cleanup_warning.as_ref() {
        emit_log_cleanup_warning("startup", warning.log_dir.as_path(), &warning.error);
    }
    if let Some(file_logging) = config.observability.file_logging.clone() {
        spawn_log_cleanup_task(config.service_name, file_logging);
    }

    Ok(Box::new(move |level: &str| {
        if let Ok(new_filter) = EnvFilter::try_new(level) {
            let _ = reload_handle.modify(|filter| *filter = new_filter);
        }
    }))
}

#[derive(Debug, Clone)]
struct RollingFileMakeWriter {
    sink: Arc<RollingFileSink>,
}

impl RollingFileMakeWriter {
    fn new(
        service_name: &'static str,
        config: FileLoggingConfig,
    ) -> Result<(Self, Option<StartupCleanupWarning>), String> {
        let (sink, startup_cleanup_warning) =
            RollingFileSink::new(service_name, config).map_err(|err| err.to_string())?;
        Ok((
            Self {
                sink: Arc::new(sink),
            },
            startup_cleanup_warning,
        ))
    }
}

impl<'a> MakeWriter<'a> for RollingFileMakeWriter {
    type Writer = RollingFileWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RollingFileWriter {
            sink: Arc::clone(&self.sink),
        }
    }
}

struct RollingFileWriter {
    sink: Arc<RollingFileSink>,
}

impl Write for RollingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sink.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}

#[derive(Debug)]
struct RollingFileSink {
    service_name: &'static str,
    config: FileLoggingConfig,
    state: Mutex<RollingFileState>,
}

#[derive(Debug)]
struct RollingFileState {
    current_bucket: String,
    file: File,
}

impl RollingFileSink {
    fn new(
        service_name: &'static str,
        config: FileLoggingConfig,
    ) -> io::Result<(Self, Option<StartupCleanupWarning>)> {
        Self::new_with_cleanup(service_name, config, cleanup_log_files)
    }

    fn new_with_cleanup(
        service_name: &'static str,
        config: FileLoggingConfig,
        cleanup: fn(&str, &FileLoggingConfig) -> io::Result<usize>,
    ) -> io::Result<(Self, Option<StartupCleanupWarning>)> {
        fs::create_dir_all(&config.dir)?;
        let startup_cleanup_warning =
            cleanup(service_name, &config)
                .err()
                .map(|err| StartupCleanupWarning {
                    log_dir: config.dir.clone(),
                    error: err.to_string(),
                });
        let now = Local::now();
        let current_bucket = log_bucket_key(config.rotation, now);
        let file = open_bucketed_log_file(&config.dir, service_name, &current_bucket)?;
        Ok((
            Self {
                service_name,
                config,
                state: Mutex::new(RollingFileState {
                    current_bucket,
                    file,
                }),
            },
            startup_cleanup_warning,
        ))
    }

    fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let now = Local::now();
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log writer mutex poisoned"))?;
        let target_bucket = log_bucket_key(self.config.rotation, now);
        if state.current_bucket != target_bucket {
            state.file.flush()?;
            state.file =
                open_bucketed_log_file(&self.config.dir, self.service_name, &target_bucket)?;
            state.current_bucket = target_bucket;
        }
        state.file.write(buf)
    }

    fn flush(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log writer mutex poisoned"))?;
        state.file.flush()
    }
}

fn open_bucketed_log_file(dir: &Path, service_name: &str, bucket: &str) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(bucketed_log_path(dir, service_name, bucket))
}

fn bucketed_log_path(dir: &Path, service_name: &str, bucket: &str) -> PathBuf {
    dir.join(format!("{service_name}.{bucket}.log"))
}

fn log_bucket_key<Tz>(rotation: LogRotation, now: DateTime<Tz>) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    match rotation {
        LogRotation::Hourly => now.format("%Y-%m-%d-%H").to_string(),
        LogRotation::Daily => now.format("%Y-%m-%d").to_string(),
    }
}

fn spawn_log_cleanup_task(service_name: &'static str, config: FileLoggingConfig) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(6 * 60 * 60);
        loop {
            tokio::time::sleep(interval).await;
            if let Err(err) = cleanup_log_files(service_name, &config) {
                emit_log_cleanup_warning("background", config.dir.as_path(), &err);
            }
        }
    });
}

fn emit_log_cleanup_warning(phase: &'static str, log_dir: &Path, error: &impl std::fmt::Display) {
    tracing::warn!(
        event_name = "log_retention_cleanup_failed",
        log_type = "ops",
        phase,
        log_dir = %log_dir.display(),
        error = %error,
        "log retention cleanup failed"
    );
}

fn cleanup_log_files(service_name: &str, config: &FileLoggingConfig) -> io::Result<usize> {
    let candidates = collect_log_file_candidates(service_name, &config.dir)?;
    let now = SystemTime::now();
    let targets =
        select_log_files_for_cleanup(candidates, now, config.retention_days, config.max_files);
    let mut removed = 0usize;
    for path in targets {
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(removed)
}

fn collect_log_file_candidates(
    service_name: &str,
    dir: &Path,
) -> io::Result<Vec<LogFileCandidate>> {
    let prefix = format!("{service_name}.");
    let mut candidates = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".log") {
            continue;
        }
        let modified_at = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        candidates.push(LogFileCandidate { path, modified_at });
    }
    Ok(candidates)
}

#[derive(Debug, Clone)]
struct LogFileCandidate {
    path: PathBuf,
    modified_at: SystemTime,
}

fn select_log_files_for_cleanup(
    mut candidates: Vec<LogFileCandidate>,
    now: SystemTime,
    retention_days: u64,
    max_files: usize,
) -> Vec<PathBuf> {
    candidates.sort_by_key(|candidate| Reverse(candidate.modified_at));
    let age_cutoff = retention_days
        .checked_mul(86_400)
        .and_then(|seconds| now.checked_sub(Duration::from_secs(seconds)));
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let exceeds_count = max_files > 0 && index >= max_files;
            let exceeds_age = age_cutoff.is_some_and(|cutoff| candidate.modified_at < cutoff);
            (exceeds_count || exceeds_age).then_some(candidate.path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        bucketed_log_path, cleanup_log_files, format_target_cell, log_bucket_key,
        select_log_files_for_cleanup, FileLoggingConfig, JsonRuntimeEventFormatter,
        LogFileCandidate, LogRotation, PrettyRuntimeEventFormatter, RollingFileSink,
        RuntimeLogIdentity,
    };
    use chrono::{Local, TimeZone};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};
    use tracing_subscriber::prelude::*;
    use uuid::Uuid;

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedBufferWriter(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().expect("buffer should lock").clone())
                .expect("buffer should contain valid utf-8")
        }
    }

    impl std::io::Write for SharedBufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("buffer should lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for SharedBuffer {
        type Writer = SharedBufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedBufferWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn cleanup_selection_respects_max_files_before_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let candidates = vec![
            LogFileCandidate {
                path: PathBuf::from("newest.log"),
                modified_at: now,
            },
            LogFileCandidate {
                path: PathBuf::from("middle.log"),
                modified_at: now - Duration::from_secs(60),
            },
            LogFileCandidate {
                path: PathBuf::from("oldest.log"),
                modified_at: now - Duration::from_secs(120),
            },
        ];

        let removed = select_log_files_for_cleanup(candidates, now, 30, 2);
        assert_eq!(removed, vec![PathBuf::from("oldest.log")]);
    }

    #[test]
    fn cleanup_selection_respects_retention_days() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20 * 86_400);
        let candidates = vec![
            LogFileCandidate {
                path: PathBuf::from("fresh.log"),
                modified_at: now - Duration::from_secs(2 * 86_400),
            },
            LogFileCandidate {
                path: PathBuf::from("expired.log"),
                modified_at: now - Duration::from_secs(10 * 86_400),
            },
        ];

        let removed = select_log_files_for_cleanup(candidates, now, 7, 0);
        assert_eq!(removed, vec![PathBuf::from("expired.log")]);
    }

    #[test]
    fn log_bucket_key_formats_daily_and_hourly_rotations() {
        let instant = Local
            .with_ymd_and_hms(2026, 4, 4, 13, 37, 0)
            .single()
            .expect("timestamp should build");

        assert_eq!(
            log_bucket_key(LogRotation::Daily, instant),
            "2026-04-04".to_string()
        );
        assert_eq!(
            log_bucket_key(LogRotation::Hourly, instant),
            "2026-04-04-13".to_string()
        );
    }

    #[test]
    fn cleanup_log_files_removes_matching_files_on_disk() {
        let dir = std::env::temp_dir().join(format!("aether-runtime-logs-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should exist");

        let file_a = bucketed_log_path(&dir, "runtime-test", "2026-04-03");
        let file_b = bucketed_log_path(&dir, "runtime-test", "2026-04-04");
        fs::write(&file_a, b"one").expect("file a should be created");
        fs::write(&file_b, b"two").expect("file b should be created");

        let config = FileLoggingConfig::new(&dir, LogRotation::Daily, 0, 0);
        let removed =
            cleanup_log_files("runtime-test", &config).expect("cleanup should succeed on disk");

        assert_eq!(removed, 2);
        assert!(!file_a.exists(), "cleanup should remove file a");
        assert!(!file_b.exists(), "cleanup should remove file b");

        fs::remove_dir_all(&dir).expect("temp dir should be removable");
    }

    #[test]
    fn rolling_file_sink_treats_startup_cleanup_failure_as_non_fatal() {
        fn fail_cleanup(_: &str, _: &FileLoggingConfig) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cleanup denied",
            ))
        }

        let dir = std::env::temp_dir().join(format!("aether-runtime-logs-{}", Uuid::new_v4()));
        let config = FileLoggingConfig::new(&dir, LogRotation::Daily, 7, 30);

        let (sink, warning) =
            RollingFileSink::new_with_cleanup("runtime-test", config, fail_cleanup)
                .expect("startup cleanup failure should not block sink creation");

        assert_eq!(sink.service_name, "runtime-test");
        let warning = warning.expect("startup cleanup warning should be surfaced");
        assert_eq!(warning.log_dir, dir);
        assert_eq!(warning.error, "cleanup denied".to_string());

        fs::remove_dir_all(&warning.log_dir).expect("temp dir should be removable");
    }

    #[test]
    fn pretty_formatter_omits_service_identity_fields() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer.clone())
                .event_format(PrettyRuntimeEventFormatter::new(
                    RuntimeLogIdentity {
                        service: "test-service",
                        node_role: Some("frontdoor".to_string()),
                        instance_id: Some("gateway-a".to_string()),
                    },
                    false,
                )),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        tracing::info!(
            target: "runtime::tracing",
            event_name = "test_event",
            value = 7_u64,
            "hello"
        );

        let output = writer.contents();
        assert!(
            !output.contains("test-service:frontdoor"),
            "should not contain compact identity"
        );
        assert!(
            output.contains(" | INFO"),
            "should contain pipe-separated level"
        );
        assert!(
            output.contains("runtime::tracing"),
            "should contain shortened target"
        );
        assert!(
            output.contains(" - hello"),
            "should contain message after dash"
        );
        assert!(output.contains("event_name=\"test_event\""));
        assert!(output.contains("value=7"));
    }

    #[test]
    fn pretty_formatter_emits_ansi_when_enabled() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer.clone())
                .event_format(PrettyRuntimeEventFormatter::new(
                    RuntimeLogIdentity {
                        service: "test-service",
                        node_role: Some("frontdoor".to_string()),
                        instance_id: Some("gateway-a".to_string()),
                    },
                    true,
                )),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        tracing::warn!(event_name = "ansi_event", "colored");

        let output = writer.contents();
        assert!(
            output.contains("\u{1b}["),
            "should contain ANSI escape sequences"
        );
        assert!(
            !output.contains("test-service:frontdoor"),
            "should not contain compact identity"
        );
        assert!(output.contains("colored"), "should contain message text");
        assert!(output.contains("ansi_event"));
    }

    #[test]
    fn pretty_formatter_adds_tree_prefix_inside_span() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer.clone())
                .event_format(PrettyRuntimeEventFormatter::new(
                    RuntimeLogIdentity {
                        service: "test-service",
                        node_role: Some("frontdoor".to_string()),
                        instance_id: Some("gateway-a".to_string()),
                    },
                    false,
                )),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        tracing::info_span!("candidates").in_scope(|| {
            tracing::debug!(
                target: "executor::candidate_loop",
                event_name = "candidate_loop_started",
                "inside span"
            );
        });

        let output = writer.contents();
        assert!(output.contains("executor::candidate_loop"));
        assert!(output.contains(" - ├─ inside span"));
    }

    #[test]
    fn pretty_formatter_keeps_target_column_aligned() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer.clone())
                .event_format(PrettyRuntimeEventFormatter::new(
                    RuntimeLogIdentity {
                        service: "test-service",
                        node_role: Some("frontdoor".to_string()),
                        instance_id: Some("gateway-a".to_string()),
                    },
                    false,
                )),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        tracing::info!(
            target: "short::name",
            event_name = "short_event",
            "short message"
        );
        tracing::info!(
            target: "root::supercalifragilistic::anotherverylongsegment",
            event_name = "long_event",
            "long message"
        );

        let output = writer.contents();
        let lines = output.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2, "expected exactly two log lines");
        let first_dash = lines[0]
            .find(" - ")
            .expect("short line should contain message separator");
        let second_dash = lines[1]
            .find(" - ")
            .expect("long line should contain message separator");
        assert_eq!(
            first_dash, second_dash,
            "message separator should stay aligned"
        );
        assert!(
            lines[1].contains(&format_target_cell(
                "root::supercalifragilistic::anotherverylongsegment",
                super::TARGET_COLUMN_WIDTH,
            )),
            "long target should be truncated into the fixed-width cell"
        );
    }

    #[test]
    fn json_formatter_includes_service_identity_fields() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(writer.clone())
                .event_format(JsonRuntimeEventFormatter::new(RuntimeLogIdentity {
                    service: "test-service",
                    node_role: Some("proxy".to_string()),
                    instance_id: Some("proxy-01".to_string()),
                })),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        tracing::warn!(
            event_name = "test_event",
            status = "failed",
            status_code = 502_u16,
            "boom"
        );

        let output = writer.contents();
        let payload: serde_json::Value =
            serde_json::from_str(output.trim()).expect("json log line should parse");
        assert_eq!(payload["service"], "test-service");
        assert_eq!(payload["node_role"], "proxy");
        assert_eq!(payload["instance_id"], "proxy-01");
        assert_eq!(payload["span_depth"], 0);
        assert_eq!(payload["fields"]["event_name"], "test_event");
        assert_eq!(payload["fields"]["status"], "failed");
        assert_eq!(payload["fields"]["status_code"], 502);
    }

    #[test]
    fn json_formatter_includes_span_depth() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(writer.clone())
                .event_format(JsonRuntimeEventFormatter::new(RuntimeLogIdentity {
                    service: "test-service",
                    node_role: Some("proxy".to_string()),
                    instance_id: Some("proxy-01".to_string()),
                })),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        tracing::info_span!("request").in_scope(|| {
            tracing::info!(event_name = "nested_event", "inside request span");
        });

        let output = writer.contents();
        let payload: serde_json::Value =
            serde_json::from_str(output.trim()).expect("json log line should parse");
        assert_eq!(payload["span_depth"], 1);
        assert_eq!(payload["fields"]["event_name"], "nested_event");
    }
}
