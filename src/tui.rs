use crate::{Host, Snapshot, SnapshotRequest, attach, discover_snapshot, snapshot_request};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    io::{self, IsTerminal, Stdout},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

pub(crate) fn run(
    socket: Option<String>,
    config: Option<PathBuf>,
    host: Option<String>,
    local: bool,
) -> ExitCode {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("error: tui requires a terminal on stdin and stdout");
        return ExitCode::from(1);
    }
    let args = TuiArgs {
        socket,
        config,
        host,
        local,
    };
    let request = match snapshot_request(
        args.socket.clone(),
        args.config.clone(),
        args.host.clone(),
        args.local,
    ) {
        Ok(request) => request,
        Err((message, code)) => {
            eprintln!("error: {message}");
            return ExitCode::from(code);
        }
    };
    match run_terminal(App::new(request)) {
        Ok(Some(selection)) => attach_selected(args, selection),
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: tui failed: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Clone)]
struct TuiArgs {
    socket: Option<String>,
    config: Option<PathBuf>,
    host: Option<String>,
    local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingAttach {
    host: String,
    session_id: String,
}

fn attach_selected(args: TuiArgs, selection: PendingAttach) -> ExitCode {
    if args.local || args.socket.is_some() {
        return attach::run(&selection.session_id, args.socket, None, None, args.local);
    }
    attach::run(
        &selection.session_id,
        None,
        args.config,
        Some(args.host.unwrap_or(selection.host)),
        false,
    )
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn run_terminal(mut app: App) -> io::Result<Option<PendingAttach>> {
    let mut terminal = TerminalGuard::enter()?;
    app.refresh();
    loop {
        terminal.terminal.draw(|frame| app.render(frame))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && let Some(action) = app.handle_key(key)
        {
            drop(terminal);
            return Ok(action.attach_selection());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Quit,
    Attach(PendingAttach),
}

impl Action {
    fn attach_selection(self) -> Option<PendingAttach> {
        match self {
            Self::Quit => None,
            Self::Attach(selection) => Some(selection),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    host: String,
    session_id: Option<String>,
    label: String,
}

struct App {
    request: SnapshotRequest,
    snapshot: Snapshot,
    rows: Vec<Row>,
    selected: usize,
    last_status: String,
}

impl App {
    fn new(request: SnapshotRequest) -> Self {
        Self {
            request,
            snapshot: Snapshot {
                schema_version: 1,
                observed_at_unix_ms: 0,
                hosts: Vec::new(),
            },
            rows: Vec::new(),
            selected: 0,
            last_status: "loading".into(),
        }
    }

    fn refresh(&mut self) {
        self.snapshot = discover_snapshot(&self.request);
        self.rows = rows(&self.snapshot);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        if self
            .rows
            .get(self.selected)
            .is_none_or(|row| row.session_id.is_none())
            && let Some(index) = self.rows.iter().position(|row| row.session_id.is_some())
        {
            self.selected = index;
        }
        let total_sessions: usize = self
            .snapshot
            .hosts
            .iter()
            .map(|host| host.sessions.len())
            .sum();
        let failed = self
            .snapshot
            .hosts
            .iter()
            .filter(|host| host.error.is_some())
            .count();
        self.last_status = format!(
            "{} hosts, {total_sessions} sessions, {failed} errors",
            self.snapshot.hosts.len()
        );
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::Quit)
            }
            KeyCode::Enter => self.selected_attach().map(Action::Attach),
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                None
            }
            KeyCode::Char('r') => {
                self.refresh();
                None
            }
            _ => None,
        }
    }

    fn selected_attach(&self) -> Option<PendingAttach> {
        let row = self.rows.get(self.selected)?;
        Some(PendingAttach {
            host: row.host.clone(),
            session_id: row.session_id.clone()?,
        })
    }

    fn move_down(&mut self) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + 1).min(self.rows.len() - 1);
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(area);

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                "Argos",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" tmux sessions"),
        ]))
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true });
        frame.render_widget(title, chunks[0]);

        let items = self
            .rows
            .iter()
            .map(|row| ListItem::new(row.label.clone()))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        if !items.is_empty() {
            state.select(Some(self.selected));
        }
        let list = List::new(items)
            .block(Block::default().title("Sessions").borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, chunks[1], &mut state);

        let selected = self
            .rows
            .get(self.selected)
            .and_then(|row| {
                row.session_id
                    .as_ref()
                    .map(|id| format!(" selected {}/{}", row.host, id))
            })
            .unwrap_or_default();
        let footer = Paragraph::new(format!(
            "{}{} | Enter attach | j/k or arrows move | r refresh | q quit",
            self.last_status, selected
        ))
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }
}

fn rows(snapshot: &Snapshot) -> Vec<Row> {
    snapshot.hosts.iter().flat_map(host_rows).collect()
}

fn host_rows(host: &Host) -> Vec<Row> {
    let mut rows = vec![Row {
        host: host.id.clone(),
        session_id: None,
        label: format!("{} ({})", host.id, host.status),
    }];
    if let Some(error) = &host.error {
        rows.push(Row {
            host: host.id.clone(),
            session_id: None,
            label: format!("  error {}: {}", error.code, error.message),
        });
        return rows;
    }
    if host.sessions.is_empty() {
        rows.push(Row {
            host: host.id.clone(),
            session_id: None,
            label: "  no tmux sessions".into(),
        });
        return rows;
    }
    rows.extend(host.sessions.iter().map(|session| Row {
        host: host.id.clone(),
        session_id: Some(session.id.clone()),
        label: format!(
            "  {}  {} windows, {} clients  {}",
            session.name, session.windows, session.attached_clients, session.id
        ),
    }));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostError, tmux};
    use std::time::UNIX_EPOCH;

    fn snapshot() -> Snapshot {
        Snapshot {
            schema_version: 1,
            observed_at_unix_ms: UNIX_EPOCH.elapsed().unwrap().as_millis() as u64,
            hosts: vec![
                Host {
                    id: "local".into(),
                    status: "ok",
                    sessions: vec![tmux::Session {
                        id: "$1".into(),
                        name: "work".into(),
                        windows: 2,
                        attached_clients: 1,
                    }],
                    error: None,
                },
                Host {
                    id: "remote".into(),
                    status: "error",
                    sessions: Vec::new(),
                    error: Some(HostError {
                        code: "timeout".into(),
                        message: "slow".into(),
                    }),
                },
            ],
        }
    }

    #[test]
    fn rows_include_hosts_sessions_and_errors() {
        let rows = rows(&snapshot());
        assert_eq!(rows[0].label, "local (ok)");
        assert!(rows.iter().any(|row| row.label.contains("work")));
        assert!(rows.iter().any(|row| row.label.contains("error timeout")));
    }

    #[test]
    fn navigation_stays_in_bounds() {
        let mut app = App::new(SnapshotRequest {
            machines: Vec::new(),
            timeout_secs: 1,
            parallel: 1,
        });
        app.snapshot = snapshot();
        app.rows = rows(&app.snapshot);
        app.move_up();
        assert_eq!(app.selected, 0);
        for _ in 0..10 {
            app.move_down();
        }
        assert_eq!(app.selected, app.rows.len() - 1);
    }
}
