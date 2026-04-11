//! Interactive TUI for configuring aether-proxy.
//!
//! Launched via `aether-proxy setup [path]`.  Presents a full-screen form
//! backed by ratatui where the user can navigate fields, edit values, and
//! save to a TOML config file.  Supports multi-server configuration via
//! a tabbed interface.

use std::io;
use std::path::PathBuf;
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

use crate::config::{ConfigFile, ServerEntry};

/// Outcome of the setup wizard, returned to the caller.
pub enum SetupOutcome {
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
                    value: "proxy-01".into(),
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
                    label: "Log Level",
                    key: "log_level",
                    value: "info".into(),
                    kind: FieldKind::LogLevel,
                    required: true,
                    help: "Log level -- Enter to cycle: trace / debug / info / warn / error",
                },
                Field {
                    label: "Log JSON",
                    key: "log_json",
                    value: "false".into(),
                    kind: FieldKind::Bool,
                    required: true,
                    help: "Output logs as JSON -- Enter to toggle",
                },
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
                    required: true,
                    help: "Install as managed service (requires root) -- Enter to toggle",
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
                "log_json" => cfg.log_json.map(|v| v.to_string()),
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

    fn to_config(&self) -> ConfigFile {
        let get_global = |key: &str| -> Option<String> {
            self.global_fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.clone())
                .filter(|v| !v.is_empty())
        };

        let get_tab = |tab: &ServerTab, key: &str| -> Option<String> {
            tab.fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.clone())
                .filter(|v| !v.is_empty())
        };

        let mut cfg = ConfigFile {
            log_level: get_global("log_level"),
            log_json: get_global("log_json").and_then(|v| v.parse().ok()),
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
        cfg
    }

    fn save(&mut self) -> anyhow::Result<()> {
        let cfg = self.to_config();
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
                "unsaved changes! q again to discard, ^S to save".into(),
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
        if self.confirm_delete && !matches!(key.code, KeyCode::Delete | KeyCode::Char('x')) {
            self.confirm_delete = false;
            self.message = None;
        }

        match key.code {
            KeyCode::Char('s')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::SUPER) =>
            {
                if let Err(e) = self.save() {
                    self.message = Some((format!("error: {}", e), Instant::now(), true));
                }
            }
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
            KeyCode::Tab => {
                if self.server_tabs.len() > 1 {
                    self.active_tab = (self.active_tab + 1) % self.server_tabs.len();
                    self.clamp_selection();
                }
            }
            KeyCode::BackTab => {
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
            KeyCode::Delete | KeyCode::Char('x') => {
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
                        "press Delete/x again to remove this server".into(),
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
                if self.validate_edit() {
                    self.selected_field_mut().value = self.edit_buffer.clone();
                    self.modified = true;
                    self.mode = Mode::Normal;
                } else {
                    self.message = Some(("invalid format".into(), Instant::now(), true));
                }
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

    fn validate_edit(&self) -> bool {
        true
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

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let help = app.selected_field().help;

    let keybindings = if app.mode == Mode::Editing {
        "Enter confirm  Esc cancel"
    } else if app.server_tabs.len() > 1 {
        "j/k select  Enter edit  Tab switch  + add  x remove  ^S save  q quit"
    } else {
        "j/k select  Enter edit  + add server  ^S save  q quit"
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
