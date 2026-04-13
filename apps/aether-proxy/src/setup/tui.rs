//! Interactive TUI for configuring aether-proxy.
//!
//! Launched via `aether-proxy setup [path]`.  Presents a full-screen form
//! backed by ratatui where the user can navigate fields, edit values, and
//! save to a TOML config file.  Supports multi-server configuration via
//! a tabbed interface.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use ratatui::Terminal;

use crate::config::{
    format_byte_size_human, parse_byte_size, ConfigFile, ProxyLogDestinationArg,
    ProxyLogRotationArg, ServerEntry, DEFAULT_HEARTBEAT_INTERVAL_SECS, DEFAULT_LOG_MAX_FILES,
    DEFAULT_LOG_RETENTION_DAYS, DEFAULT_REDIRECT_REPLAY_BUDGET_HUMAN,
};

/// Outcome of the setup wizard, returned to the caller.
pub enum SetupOutcome {
    /// Config saved and the selected host service was installed.
    ServiceInstalled,
    /// Config saved; no service -- caller should start the proxy directly.
    ReadyToRun(PathBuf),
    /// User quit without saving.
    Cancelled,
}

/// Column width reserved for the field label (chars).
const LABEL_WIDTH: usize = 22;

// -- Field types --------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum FieldKind {
    Text,
    Secret,
    Bool,
    LogLevel,
}

struct Field {
    label: &'static str,
    key: &'static str,
    value: String,
    kind: FieldKind,
    required: bool,
    help: &'static str,
}
// -- Server tab ---------------------------------------------------------------

/// A single server tab's editable fields.
struct ServerTab {
    fields: Vec<Field>,
}

impl ServerTab {
    fn new() -> Self {
        Self {
            fields: vec![
                Field {
                    label: "Aether URL",
                    key: "aether_url",
                    value: String::new(),
                    kind: FieldKind::Text,
                    required: true,
                    help: "Aether URL (e.g. https://aether.example.com)",
                },
                Field {
                    label: "Management Token",
                    key: "management_token",
                    value: String::new(),
                    kind: FieldKind::Secret,
                    required: true,
                    help: "Aether Management Token (ae_xxx)",
                },
                Field {
                    label: "Node Name",
                    key: "node_name",
                    value: String::new(),
                    kind: FieldKind::Text,
                    required: true,
                    help: "Node name for identification in Aether dashboard",
                },
            ],
        }
    }

    fn from_entry(entry: &ServerEntry) -> Self {
        let mut tab = Self::new();
        tab.fields[0].value = entry.aether_url.clone();
        tab.fields[1].value = entry.management_token.clone();
        if let Some(ref name) = entry.node_name {
            tab.fields[2].value = name.clone();
        }
        tab
    }
}

// -- App state ----------------------------------------------------------------

#[derive(PartialEq)]
enum Mode {
    Normal,
    Editing,
}

struct App {
    server_tabs: Vec<ServerTab>,
    active_tab: usize,
    global_fields: Vec<Field>,
    selected: usize,
    mode: Mode,
    edit_buffer: String,
    edit_cursor: usize,
    config_path: PathBuf,
    modified: bool,
    message: Option<(String, Instant, bool)>,
    scroll_offset: usize,
    saved_once: bool,
    pending_quit: bool,
    confirm_delete: bool,
}
impl App {
    fn new(config_path: PathBuf) -> Self {
        Self {
            server_tabs: vec![ServerTab::new()],
            active_tab: 0,
            global_fields: vec![
                Field {
                    label: "Install Service",
                    key: "install_service",
                    value: if super::service::is_available() {
                        "true"
                    } else {
                        "false"
                    }
                    .into(),
                    kind: FieldKind::Bool,
                    required: false,
                    help: "Install as managed service (requires root) -- Enter to toggle",
                },
                Field {
                    label: "Log Level",
                    key: "log_level",
                    value: "info".into(),
                    kind: FieldKind::LogLevel,
                    required: true,
                    help: "Log level -- Enter to cycle: trace / debug / info / warn / error",
                },
                Field {
                    label: "Save Logs to File",
                    key: "save_logs_to_file",
                    value: "false".into(),
                    kind: FieldKind::Bool,
                    required: false,
                    help: "Write pretty .log files with daily rotation and 7-day retention",
                },
                Field {
                    label: "Heartbeat Interval",
                    key: "heartbeat_interval",
                    value: DEFAULT_HEARTBEAT_INTERVAL_SECS.to_string(),
                    kind: FieldKind::Text,
                    required: false,
                    help: "Heartbeat interval in seconds; default is 30",
                },
                Field {
                    label: "Redirect Replay Budget",
                    key: "redirect_replay_budget_bytes",
                    value: DEFAULT_REDIRECT_REPLAY_BUDGET_HUMAN.to_string(),
                    kind: FieldKind::Text,
                    required: false,
                    help:
                        "Prebuffer budget for 307/308 replay, e.g. 5M; set 0 to disable buffering",
                },
            ],
            selected: 0,
            mode: Mode::Normal,
            edit_buffer: String::new(),
            edit_cursor: 0,
            config_path,
            modified: false,
            message: None,
            scroll_offset: 0,
            saved_once: false,
            pending_quit: false,
            confirm_delete: false,
        }
    }

    // -- Field accessors (unified index across server + global) ---------------

    fn server_field_count(&self) -> usize {
        self.server_tabs[self.active_tab].fields.len()
    }

    fn total_field_count(&self) -> usize {
        self.server_field_count() + self.global_fields.len()
    }

    fn selected_field(&self) -> &Field {
        let sc = self.server_field_count();
        if self.selected < sc {
            &self.server_tabs[self.active_tab].fields[self.selected]
        } else {
            &self.global_fields[self.selected - sc]
        }
    }

    fn selected_field_mut(&mut self) -> &mut Field {
        let sc = self.server_field_count();
        if self.selected < sc {
            &mut self.server_tabs[self.active_tab].fields[self.selected]
        } else {
            &mut self.global_fields[self.selected - sc]
        }
    }

    fn clamp_selection(&mut self) {
        let max = self.total_field_count();
        if self.selected >= max {
            self.selected = max.saturating_sub(1);
        }
        self.scroll_offset = 0;
        self.confirm_delete = false;
    }
    // -- Config <-> fields -----------------------------------------------------

    fn load_from_file(&mut self) {
        if let Ok(cfg) = ConfigFile::load(&self.config_path) {
            self.apply_config(&cfg);
        }
    }

    fn apply_config(&mut self, cfg: &ConfigFile) {
        // Global fields
        for field in &mut self.global_fields {
            let val: Option<String> = match field.key {
                "log_level" => cfg.log_level.clone(),
                "save_logs_to_file" => cfg.log_destination.map(|value| {
                    matches!(
                        value,
                        ProxyLogDestinationArg::File | ProxyLogDestinationArg::Both
                    )
                    .to_string()
                }),
                "heartbeat_interval" => cfg.heartbeat_interval.map(|v| v.to_string()),
                "redirect_replay_budget_bytes" => cfg.redirect_replay_budget_bytes.clone(),
                _ => None,
            };
            if let Some(v) = val {
                field.value = v;
            }
        }

        // Server tabs
        let servers = cfg.effective_servers();
        if servers.is_empty() {
            let mut tab = ServerTab::new();
            // Single-server fallback: use top-level node_name
            if let Some(ref name) = cfg.node_name {
                tab.fields[2].value = name.clone();
            }
            self.server_tabs = vec![tab];
        } else {
            self.server_tabs = servers.iter().map(ServerTab::from_entry).collect();
            // For single-server mode, node_name might be in top-level only
            if self.server_tabs.len() == 1 && self.server_tabs[0].fields[2].value.is_empty() {
                if let Some(ref name) = cfg.node_name {
                    self.server_tabs[0].fields[2].value = name.clone();
                }
            }
        }
        self.active_tab = 0;
        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn get_global(&self, key: &str) -> Option<String> {
        self.global_fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .filter(|v| !v.is_empty())
    }

    fn get_tab(tab: &ServerTab, key: &str) -> Option<String> {
        tab.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .filter(|v| !v.is_empty())
    }

    fn toggle_enabled(&self, key: &str) -> bool {
        self.get_global(key).as_deref() == Some("true")
    }

    fn validate_required_fields(&self) -> anyhow::Result<()> {
        for (tab_idx, tab) in self.server_tabs.iter().enumerate() {
            for field in &tab.fields {
                if field.required && field.value.trim().is_empty() {
                    anyhow::bail!("server {} field `{}` is required", tab_idx + 1, field.label);
                }
            }
        }

        for field in &self.global_fields {
            if field.required && field.value.trim().is_empty() {
                anyhow::bail!("field `{}` is required", field.label);
            }
        }

        Ok(())
    }

    fn parse_optional_heartbeat_interval(&self) -> anyhow::Result<Option<u64>> {
        let Some(raw) = self.get_global("heartbeat_interval") else {
            return Ok(None);
        };
        let value = raw.trim().parse::<u64>().map_err(|_| {
            anyhow::anyhow!("heartbeat interval must be an integer number of seconds")
        })?;
        if value == 0 || value > 3600 {
            anyhow::bail!("heartbeat interval must be between 1 and 3600 seconds");
        }
        Ok(Some(value))
    }

    fn parse_optional_redirect_replay_budget(&self) -> anyhow::Result<Option<String>> {
        let Some(raw) = self.get_global("redirect_replay_budget_bytes") else {
            return Ok(None);
        };
        let bytes = parse_byte_size(raw.trim())
            .map_err(|err| anyhow::anyhow!("redirect replay budget invalid: {err}"))?;
        Ok(Some(format_byte_size_human(bytes)))
    }

    fn default_file_log_dir(&self) -> String {
        if self.toggle_enabled("install_service") {
            return "/var/log/aether-proxy".to_string();
        }

        let base = self
            .config_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if base == Path::new(".") {
            "logs".to_string()
        } else {
            base.join("logs").display().to_string()
        }
    }

    fn to_config(&self) -> anyhow::Result<ConfigFile> {
        self.validate_required_fields()?;

        let get_global = |key: &str| -> Option<String> { self.get_global(key) };

        let get_tab = |tab: &ServerTab, key: &str| -> Option<String> { Self::get_tab(tab, key) };

        let save_logs_to_file = self.toggle_enabled("save_logs_to_file");
        let mut cfg = ConfigFile {
            log_level: get_global("log_level"),
            heartbeat_interval: self.parse_optional_heartbeat_interval()?,
            redirect_replay_budget_bytes: self.parse_optional_redirect_replay_budget()?,
            log_destination: Some(if save_logs_to_file {
                ProxyLogDestinationArg::Both
            } else {
                ProxyLogDestinationArg::Stdout
            }),
            log_dir: save_logs_to_file.then(|| self.default_file_log_dir()),
            log_rotation: save_logs_to_file.then_some(ProxyLogRotationArg::Daily),
            log_retention_days: save_logs_to_file.then_some(DEFAULT_LOG_RETENTION_DAYS),
            log_max_files: save_logs_to_file.then_some(DEFAULT_LOG_MAX_FILES),
            ..ConfigFile::default()
        };

        // Always write [[servers]] format; old top-level fields are read-only compat
        cfg.servers = self
            .server_tabs
            .iter()
            .map(|tab| ServerEntry {
                aether_url: get_tab(tab, "aether_url").unwrap_or_default(),
                management_token: get_tab(tab, "management_token").unwrap_or_default(),
                node_name: get_tab(tab, "node_name"),
            })
            .collect();
        Ok(cfg)
    }

    fn save(&mut self) -> anyhow::Result<()> {
        let cfg = self.to_config()?;
        cfg.save(&self.config_path)?;
        // Restrict config file permissions to owner-only (contains management token).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.config_path, std::fs::Permissions::from_mode(0o600));
        }
        self.modified = false;
        self.saved_once = true;
        self.message = Some((
            format!("saved to {}", self.config_path.display()),
            Instant::now(),
            false,
        ));
        Ok(())
    }
    // -- Scrolling ---------------------------------------------------------------

    fn ensure_visible(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        // Account for separator line between server and global fields
        let display_row = if self.selected >= self.server_field_count() {
            self.selected + 1
        } else {
            self.selected
        };
        if display_row < self.scroll_offset {
            self.scroll_offset = display_row;
        } else if display_row >= self.scroll_offset + visible_rows {
            self.scroll_offset = display_row - visible_rows + 1;
        }
    }

    // -- Key handling -------------------------------------------------------------

    /// Returns `true` when the app should exit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Expire old messages (but keep quit-confirmation messages alive)
        if let Some((_, when, _)) = &self.message {
            if !self.pending_quit && !self.confirm_delete && when.elapsed() > Duration::from_secs(4)
            {
                self.message = None;
            }
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }

        if key.code == KeyCode::Char('s')
            && (key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER))
        {
            if self.mode == Mode::Editing && !self.commit_edit_buffer() {
                return false;
            }
            if let Err(e) = self.save() {
                self.message = Some((format!("error: {}", e), Instant::now(), true));
                return false;
            }
            return true;
        }

        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Editing => {
                self.handle_edit(key);
                false
            }
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) -> bool {
        // -- Quit handling (with unsaved-changes confirmation) -----------------
        let is_quit_key = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc);

        if is_quit_key {
            if !self.modified || self.pending_quit {
                return true;
            }
            self.pending_quit = true;
            self.confirm_delete = false;
            self.message = Some((
                "unsaved changes! q again to discard, Ctrl+S to save and exit".into(),
                Instant::now(),
                true,
            ));
            return false;
        }

        // Any other key cancels pending quit / pending delete
        if self.pending_quit {
            self.pending_quit = false;
            self.message = None;
        }
        if self.confirm_delete
            && !matches!(
                key.code,
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('x')
            )
        {
            self.confirm_delete = false;
            self.message = None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.total_field_count() {
                    self.selected += 1;
                }
            }
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.total_field_count() - 1,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let kind = self.selected_field().kind;
                let key_str = self.selected_field().key;
                let value = self.selected_field().value.clone();
                match kind {
                    FieldKind::Bool => {
                        let toggled = if value == "true" { "false" } else { "true" };
                        if key_str == "install_service"
                            && toggled == "true"
                            && !super::service::is_available()
                        {
                            self.message =
                                Some((super::service::unavailable_hint(), Instant::now(), true));
                        } else {
                            self.selected_field_mut().value = toggled.into();
                            self.modified = true;
                        }
                    }
                    FieldKind::LogLevel => {
                        const LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];
                        let idx = LEVELS.iter().position(|l| *l == value).unwrap_or(2);
                        self.selected_field_mut().value = LEVELS[(idx + 1) % LEVELS.len()].into();
                        self.modified = true;
                    }
                    _ => {
                        self.edit_buffer = value;
                        self.edit_cursor = self.edit_buffer.chars().count();
                        self.mode = Mode::Editing;
                    }
                }
            }
            // -- Tab navigation --
            KeyCode::Tab | KeyCode::Right => {
                if self.server_tabs.len() > 1 {
                    self.active_tab = (self.active_tab + 1) % self.server_tabs.len();
                    self.clamp_selection();
                }
            }
            KeyCode::BackTab | KeyCode::Left => {
                if self.server_tabs.len() > 1 {
                    self.active_tab = if self.active_tab == 0 {
                        self.server_tabs.len() - 1
                    } else {
                        self.active_tab - 1
                    };
                    self.clamp_selection();
                }
            }
            KeyCode::Char(c @ '1'..='9') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.server_tabs.len() && idx != self.active_tab {
                    self.active_tab = idx;
                    self.clamp_selection();
                }
            }
            // -- Add / remove server --
            KeyCode::Char('+') | KeyCode::Char('a') => {
                self.server_tabs.push(ServerTab::new());
                self.active_tab = self.server_tabs.len() - 1;
                self.selected = 0;
                self.scroll_offset = 0;
                self.modified = true;
                self.message = Some((
                    format!("added server {}", self.server_tabs.len()),
                    Instant::now(),
                    false,
                ));
            }
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('x') => {
                if self.server_tabs.len() <= 1 {
                    self.message =
                        Some(("cannot remove the last server".into(), Instant::now(), true));
                } else if self.confirm_delete {
                    let removed = self.active_tab + 1;
                    self.server_tabs.remove(self.active_tab);
                    self.active_tab = self.active_tab.min(self.server_tabs.len() - 1);
                    self.clamp_selection();
                    self.modified = true;
                    self.message =
                        Some((format!("server {} removed", removed), Instant::now(), false));
                } else {
                    self.confirm_delete = true;
                    self.message = Some((
                        "press Backspace again to remove this server".into(),
                        Instant::now(),
                        true,
                    ));
                }
            }
            _ => {}
        }
        false
    }

    fn handle_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                self.commit_edit_buffer();
            }
            KeyCode::Backspace => {
                if self.edit_cursor > 0 {
                    self.edit_cursor -= 1;
                    let byte = self.char_byte_pos(self.edit_cursor);
                    self.edit_buffer.remove(byte);
                }
            }
            KeyCode::Delete => {
                if self.edit_cursor < self.edit_buffer.chars().count() {
                    let byte = self.char_byte_pos(self.edit_cursor);
                    self.edit_buffer.remove(byte);
                }
            }
            KeyCode::Left => {
                self.edit_cursor = self.edit_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let len = self.edit_buffer.chars().count();
                if self.edit_cursor < len {
                    self.edit_cursor += 1;
                }
            }
            KeyCode::Home => self.edit_cursor = 0,
            KeyCode::End => self.edit_cursor = self.edit_buffer.chars().count(),
            KeyCode::Char(c) => {
                let byte = self.char_byte_pos(self.edit_cursor);
                self.edit_buffer.insert(byte, c);
                self.edit_cursor += 1;
            }
            _ => {}
        }
    }

    fn commit_edit_buffer(&mut self) -> bool {
        if self.validate_edit() {
            self.selected_field_mut().value = self.edit_buffer.clone();
            self.modified = true;
            self.mode = Mode::Normal;
            true
        } else {
            self.message = Some(("invalid format".into(), Instant::now(), true));
            false
        }
    }

    fn validate_edit(&self) -> bool {
        let key = self.selected_field().key;
        let trimmed = self.edit_buffer.trim();

        if self.selected_field().required && trimmed.is_empty() {
            return false;
        }

        match key {
            "heartbeat_interval" => {
                if trimmed.is_empty() {
                    return true;
                }
                match trimmed.parse::<u64>() {
                    Ok(value) => (1..=3600).contains(&value),
                    Err(_) => false,
                }
            }
            "redirect_replay_budget_bytes" => {
                if trimmed.is_empty() {
                    return true;
                }
                parse_byte_size(trimmed).is_ok()
            }
            _ => true,
        }
    }

    /// Byte offset of the char at `char_idx`.
    fn char_byte_pos(&self, char_idx: usize) -> usize {
        self.edit_buffer
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.edit_buffer.len())
    }
}
// -- Rendering ----------------------------------------------------------------

fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let title = if app.modified {
        " Aether Proxy Setup [*] "
    } else {
        " Aether Proxy Setup "
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_alignment(ratatui::layout::Alignment::Center)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // Split: fields | tab bar | footer
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(4),
    ])
    .split(inner);

    render_fields(f, app, chunks[0]);
    render_tab_bar(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
}

fn render_fields(f: &mut Frame, app: &mut App, area: Rect) {
    let visible = area.height as usize;
    app.ensure_visible(visible);

    let server_count = app.server_field_count();
    let mut lines: Vec<Line> = Vec::new();
    // display_row tracks the actual row index (including separator)
    let mut display_row: usize = 0;

    // Server fields
    for i in 0..server_count {
        if display_row >= app.scroll_offset && display_row < app.scroll_offset + visible {
            lines.push(build_field_line(app, i, display_row));
        }
        display_row += 1;
    }

    // Separator line
    if display_row >= app.scroll_offset && display_row < app.scroll_offset + visible {
        lines.push(Line::from(Span::styled(
            "   ----------------------------------------",
            Style::default().fg(Color::DarkGray),
        )));
    }
    display_row += 1;

    // Global fields
    for i in 0..app.global_fields.len() {
        let field_idx = server_count + i;
        if display_row >= app.scroll_offset && display_row < app.scroll_offset + visible {
            lines.push(build_field_line(app, field_idx, display_row));
        }
        display_row += 1;
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);

    // Cursor position while editing
    if app.mode == Mode::Editing {
        let sel_display_row = if app.selected >= server_count {
            app.selected + 1
        } else {
            app.selected
        };
        let row_in_view = sel_display_row.saturating_sub(app.scroll_offset);
        let prefix: u16 = 3 + LABEL_WIDTH as u16 + 2;
        let cx = area.x + prefix + app.edit_cursor as u16;
        let cy = area.y + row_in_view as u16;
        if cx < area.x + area.width && cy < area.y + area.height {
            f.set_cursor_position((cx, cy));
        }
    }
}
fn build_field_line(app: &App, field_idx: usize, _display_row: usize) -> Line<'static> {
    let sc = app.server_field_count();
    let field = if field_idx < sc {
        &app.server_tabs[app.active_tab].fields[field_idx]
    } else {
        &app.global_fields[field_idx - sc]
    };

    let selected = field_idx == app.selected;
    let indicator = if selected { " > " } else { "   " };

    let label_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let padded_label = format!("{:<width$}", field.label, width = LABEL_WIDTH);

    let (value_text, value_style) = if app.mode == Mode::Editing && selected {
        (app.edit_buffer.clone(), Style::default().fg(Color::Yellow))
    } else {
        field_display(field)
    };

    Line::from(vec![
        Span::styled(indicator.to_string(), label_style),
        Span::styled(padded_label, label_style),
        Span::raw("  "),
        Span::styled(value_text, value_style),
    ])
}

/// Returns (display_text, style) for a field in normal mode.
fn field_display(field: &Field) -> (String, Style) {
    if field.value.is_empty() {
        let text = if field.required {
            "(required)".into()
        } else {
            "-".into()
        };
        let color = if field.required {
            Color::Red
        } else {
            Color::DarkGray
        };
        return (text, Style::default().fg(color));
    }

    match field.kind {
        FieldKind::Secret => (
            "*".repeat(field.value.len().min(20)),
            Style::default().fg(Color::White),
        ),
        FieldKind::Bool => {
            if field.value == "true" {
                ("[x] on".into(), Style::default().fg(Color::Green))
            } else {
                ("[ ] off".into(), Style::default().fg(Color::DarkGray))
            }
        }
        FieldKind::LogLevel => {
            let color = match field.value.as_str() {
                "trace" => Color::Magenta,
                "debug" => Color::Blue,
                "info" => Color::Green,
                "warn" => Color::Yellow,
                "error" => Color::Red,
                _ => Color::White,
            };
            (field.value.clone(), Style::default().fg(color))
        }
        _ => (field.value.clone(), Style::default().fg(Color::White)),
    }
}
fn render_tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(" "));

    for (i, tab) in app.server_tabs.iter().enumerate() {
        let num = i + 1;
        let name = tab
            .fields
            .iter()
            .find(|f| f.key == "node_name")
            .filter(|f| !f.value.is_empty())
            .map(|f| f.value.clone())
            .unwrap_or_else(|| format!("Server {}", num));

        let label = format!(" {} {} ", num, name);

        if i == app.active_tab {
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::raw(" "));
    }

    spans.push(Span::styled(" + Add ", Style::default().fg(Color::Green)));
    if app.server_tabs.len() > 1 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            " Backspace Remove ",
            Style::default().fg(Color::Yellow),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let help = app.selected_field().help;

    let keybindings = if app.mode == Mode::Editing {
        "Enter 确认  Esc 取消  Ctrl+S 保存退出  Ctrl+C 退出"
    } else if app.server_tabs.len() > 1 {
        "↑/↓ 选择  ←/→ 切换服务器  Enter 编辑  + 新增  Backspace 删除  Ctrl+S 保存退出  Ctrl+C 退出"
    } else {
        "↑/↓ 选择  Enter 编辑  + 新增服务器  Ctrl+S 保存退出  Ctrl+C 退出"
    };

    let mut status_spans: Vec<Span> = vec![Span::styled(
        format!(" {}", keybindings),
        Style::default().fg(Color::DarkGray),
    )];

    if let Some((msg, _, is_err)) = &app.message {
        let color = if *is_err { Color::Red } else { Color::Green };
        status_spans.push(Span::raw("    "));
        status_spans.push(Span::styled(msg.clone(), Style::default().fg(color)));
    }

    let footer_text = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!(" {}", help),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(status_spans),
    ];

    let footer = Paragraph::new(footer_text).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(footer, area);
}
// -- Entry point --------------------------------------------------------------

pub fn run(config_path: PathBuf) -> anyhow::Result<SetupOutcome> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config_path.clone());
    app.load_from_file();

    let result = event_loop(&mut terminal, &mut app);

    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result?;

    // -- Post-TUI: decide outcome ---------------------------------------------

    if !app.saved_once {
        return Ok(SetupOutcome::Cancelled);
    }

    eprintln!();
    eprintln!("  Config saved to {}", config_path.display());
    eprintln!();

    let wants_service = app
        .global_fields
        .iter()
        .find(|f| f.key == "install_service")
        .map(|f| f.value == "true")
        .unwrap_or(false);

    if wants_service {
        match super::service::install_service(&config_path) {
            Ok(()) => return Ok(SetupOutcome::ServiceInstalled),
            Err(e) => {
                eprintln!("  Service install failed: {}", e);
                eprintln!("  Starting proxy directly instead.\n");
            }
        }
    } else if super::service::is_installed() {
        if let Err(e) = super::service::uninstall_service() {
            eprintln!("  Service uninstall failed: {}", e);
            eprintln!();
        }
    }

    Ok(SetupOutcome::ReadyToRun(config_path))
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && app.handle_key(key) {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn set_server_field(app: &mut App, key: &str, value: &str) {
        let field = app.server_tabs[0]
            .fields
            .iter_mut()
            .find(|field| field.key == key)
            .expect("server field");
        field.value = value.to_string();
    }

    fn set_global_field(app: &mut App, key: &str, value: &str) {
        let field = app
            .global_fields
            .iter_mut()
            .find(|field| field.key == key)
            .expect("global field");
        field.value = value.to_string();
    }

    fn sample_app() -> App {
        let mut app = App::new(PathBuf::from("aether-proxy.toml"));
        set_server_field(&mut app, "aether_url", "https://aether.example.com");
        set_server_field(&mut app, "management_token", "ae_test");
        set_server_field(&mut app, "node_name", "jp-proxy-01");
        app
    }

    fn unique_temp_config_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        std::env::temp_dir().join(format!("aether-proxy-{name}-{nanos}.toml"))
    }

    #[test]
    fn new_app_shows_default_heartbeat_interval() {
        let app = sample_app();
        let heartbeat = app
            .global_fields
            .iter()
            .find(|field| field.key == "heartbeat_interval")
            .expect("heartbeat field");
        assert_eq!(heartbeat.value, DEFAULT_HEARTBEAT_INTERVAL_SECS.to_string());
    }

    #[test]
    fn to_config_enables_pretty_file_logging_with_defaults() {
        let mut app = sample_app();
        set_global_field(&mut app, "save_logs_to_file", "true");

        let cfg = app.to_config().expect("config should serialize");
        assert_eq!(cfg.log_destination, Some(ProxyLogDestinationArg::Both));
        assert_eq!(cfg.log_dir.as_deref(), Some("logs"));
        assert_eq!(cfg.log_rotation, Some(ProxyLogRotationArg::Daily));
        assert_eq!(cfg.log_retention_days, Some(DEFAULT_LOG_RETENTION_DAYS));
        assert_eq!(cfg.log_max_files, Some(DEFAULT_LOG_MAX_FILES));
    }

    #[test]
    fn to_config_uses_service_log_dir_when_installing_service() {
        let mut app = sample_app();
        set_global_field(&mut app, "install_service", "true");
        set_global_field(&mut app, "save_logs_to_file", "true");

        let cfg = app.to_config().expect("config should serialize");
        assert_eq!(cfg.log_dir.as_deref(), Some("/var/log/aether-proxy"));
    }

    #[test]
    fn to_config_persists_optional_heartbeat_interval() {
        let mut app = sample_app();
        set_global_field(&mut app, "heartbeat_interval", "45");
        set_global_field(&mut app, "redirect_replay_budget_bytes", "6m");

        let cfg = app.to_config().expect("config should serialize");
        assert_eq!(cfg.heartbeat_interval, Some(45));
        assert_eq!(cfg.redirect_replay_budget_bytes.as_deref(), Some("6M"));
    }

    #[test]
    fn to_config_rejects_invalid_heartbeat_interval() {
        let mut app = sample_app();
        set_global_field(&mut app, "heartbeat_interval", "0");

        let error = app.to_config().expect_err("heartbeat 0 should be rejected");
        assert!(error.to_string().contains("heartbeat interval"));
    }

    #[test]
    fn to_config_rejects_missing_required_node_name() {
        let mut app = sample_app();
        set_server_field(&mut app, "node_name", "");

        let error = app
            .to_config()
            .expect_err("missing node_name should be rejected");
        assert!(error.to_string().contains("Node Name"));
    }

    #[test]
    fn ctrl_c_exits_immediately_even_with_unsaved_changes() {
        let mut app = sample_app();
        app.modified = true;

        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL,)));
        assert!(!app.saved_once);
    }

    #[test]
    fn ctrl_s_commits_edit_saves_and_exits() {
        let config_path = unique_temp_config_path("ctrl-s");
        let mut app = sample_app();
        app.config_path = config_path.clone();
        app.selected = app.server_field_count() + 3;
        app.mode = Mode::Editing;
        app.edit_buffer = "45".to_string();
        app.edit_cursor = 2;

        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL,)));
        assert!(app.saved_once);
        assert!(matches!(app.mode, Mode::Normal));

        let saved = fs::read_to_string(&config_path).expect("config should be saved");
        assert!(saved.contains("heartbeat_interval = 45"));

        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn backspace_confirms_and_removes_active_server_tab() {
        let mut app = sample_app();
        app.server_tabs.push(ServerTab::from_entry(&ServerEntry {
            aether_url: "https://aether-2.example.com".to_string(),
            management_token: "ae_test_2".to_string(),
            node_name: Some("jp-proxy-02".to_string()),
        }));
        app.active_tab = 1;

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE,)));
        assert!(app.confirm_delete);
        assert_eq!(app.server_tabs.len(), 2);

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE,)));
        assert_eq!(app.server_tabs.len(), 1);
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn left_and_right_switch_between_server_tabs() {
        let mut app = sample_app();
        app.server_tabs.push(ServerTab::from_entry(&ServerEntry {
            aether_url: "https://aether-2.example.com".to_string(),
            management_token: "ae_test_2".to_string(),
            node_name: Some("jp-proxy-02".to_string()),
        }));
        app.active_tab = 0;

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE,)));
        assert_eq!(app.active_tab, 1);

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE,)));
        assert_eq!(app.active_tab, 0);
    }
}
