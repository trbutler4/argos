use crate::{Host, Snapshot, SnapshotRequest, discover_snapshot, sessions, snapshot_request, vm};
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
    collections::{BTreeMap, BTreeSet},
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
    match run_terminal(App::new(request, args.clone())) {
        Ok(Some(attach)) => attach_selected(attach),
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: tui failed: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TuiArgs {
    socket: Option<String>,
    config: Option<PathBuf>,
    host: Option<String>,
    local: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAttach {
    target: String,
    socket: Option<String>,
    config_path: Option<PathBuf>,
    host: Option<String>,
    local: bool,
    state_dir: Option<PathBuf>,
}

fn attach_selected(attach: PendingAttach) -> ExitCode {
    sessions::attach(sessions::AttachOptions {
        target: attach.target,
        socket: attach.socket,
        config_path: attach.config_path,
        host: attach.host,
        local: attach.local,
        state_dir: attach.state_dir,
    })
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
            match action {
                Action::Quit => return Ok(None),
                Action::Attach(attach) => return Ok(Some(attach)),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Quit,
    Attach(PendingAttach),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    host: String,
    attach: Option<PendingAttach>,
    label: String,
}

struct App {
    request: SnapshotRequest,
    args: TuiArgs,
    snapshot: Snapshot,
    vm_snapshot: Option<vm::VmSnapshot>,
    vm_error: Option<String>,
    rows: Vec<Row>,
    selected: usize,
    last_status: String,
}

impl App {
    fn new(request: SnapshotRequest, args: TuiArgs) -> Self {
        Self {
            request,
            args,
            snapshot: Snapshot {
                schema_version: 1,
                observed_at_unix_ms: 0,
                hosts: Vec::new(),
            },
            vm_snapshot: None,
            vm_error: None,
            rows: Vec::new(),
            selected: 0,
            last_status: "loading".into(),
        }
    }

    fn refresh(&mut self) {
        self.snapshot = discover_snapshot(&self.request);
        match vm::snapshot(None) {
            Ok(snapshot) => {
                self.vm_snapshot = Some(snapshot);
                self.vm_error = None;
            }
            Err(error) => {
                self.vm_snapshot = None;
                self.vm_error = Some(error);
            }
        }
        self.rows = rows(
            &self.snapshot,
            self.vm_snapshot.as_ref(),
            self.vm_error.as_deref(),
            &self.args,
        );
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        if self
            .rows
            .get(self.selected)
            .is_none_or(|row| row.attach.is_none())
            && let Some(index) = self.rows.iter().position(|row| row.attach.is_some())
        {
            self.selected = index;
        }
        let total_sessions: usize = self
            .snapshot
            .hosts
            .iter()
            .map(|host| host.sessions.len())
            .sum();
        let total_vms = self
            .vm_snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.vms.len());
        let failed = self
            .snapshot
            .hosts
            .iter()
            .filter(|host| host.error.is_some())
            .count()
            + usize::from(self.vm_error.is_some());
        self.last_status = format!(
            "{} hosts, {total_sessions} sessions, {total_vms} vms, {failed} errors",
            self.snapshot.hosts.len()
        );
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::Quit)
            }
            KeyCode::Enter => self
                .rows
                .get(self.selected)
                .and_then(|row| row.attach.clone())
                .map(Action::Attach),
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
            Span::raw(" sessions and VMs"),
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
            .block(Block::default().title("Hosts").borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, chunks[1], &mut state);

        let selected = self
            .rows
            .get(self.selected)
            .and_then(|row| {
                row.attach
                    .as_ref()
                    .map(|attach| format!(" selected {}", attach.target))
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

fn rows(
    snapshot: &Snapshot,
    vm_snapshot: Option<&vm::VmSnapshot>,
    vm_error: Option<&str>,
    args: &TuiArgs,
) -> Vec<Row> {
    let mut vms_by_host: BTreeMap<String, Vec<&vm::VmRecord>> = BTreeMap::new();
    if let Some(vm_snapshot) = vm_snapshot {
        for record in &vm_snapshot.vms {
            vms_by_host
                .entry(record.host.clone())
                .or_default()
                .push(record);
        }
    }
    let mut rendered_hosts = BTreeSet::new();
    let mut rows = Vec::new();
    for host in &snapshot.hosts {
        rendered_hosts.insert(host.id.clone());
        rows.extend(host_rows(
            host,
            vms_by_host.remove(&host.id).unwrap_or_default(),
            args,
        ));
    }
    for (host, records) in vms_by_host {
        if rendered_hosts.insert(host.clone()) {
            rows.extend(vm_only_host_rows(&host, records, args));
        }
    }
    if let Some(error) = vm_error {
        rows.push(Row {
            host: "vms".into(),
            attach: None,
            label: format!("VM state error: {error}"),
        });
    }
    rows
}

fn host_rows(host: &Host, vms: Vec<&vm::VmRecord>, args: &TuiArgs) -> Vec<Row> {
    let mut rows = vec![Row {
        host: host.id.clone(),
        attach: None,
        label: format!("{} ({})", host.id, host.status),
    }];
    if let Some(error) = &host.error {
        rows.push(Row {
            host: host.id.clone(),
            attach: None,
            label: format!("  sessions error {}: {}", error.code, error.message),
        });
    } else {
        rows.push(Row {
            host: host.id.clone(),
            attach: None,
            label: "  sessions".into(),
        });
        if host.sessions.is_empty() {
            rows.push(Row {
                host: host.id.clone(),
                attach: None,
                label: "    no tmux sessions".into(),
            });
        } else {
            rows.extend(host.sessions.iter().map(|session| Row {
                host: host.id.clone(),
                attach: Some(host_attach(&host.id, &session.id, args)),
                label: format!(
                    "    {}  {} windows, {} clients  {}",
                    session.name, session.windows, session.attached_clients, session.id
                ),
            }));
        }
    }
    rows.extend(vm_section_rows(&host.id, vms, args));
    rows
}

fn vm_only_host_rows(host: &str, vms: Vec<&vm::VmRecord>, args: &TuiArgs) -> Vec<Row> {
    let mut rows = vec![Row {
        host: host.into(),
        attach: None,
        label: format!("{host} (vm state)"),
    }];
    rows.push(Row {
        host: host.into(),
        attach: None,
        label: "  sessions".into(),
    });
    rows.push(Row {
        host: host.into(),
        attach: None,
        label: "    not discovered".into(),
    });
    rows.extend(vm_section_rows(host, vms, args));
    rows
}

fn vm_section_rows(host: &str, vms: Vec<&vm::VmRecord>, args: &TuiArgs) -> Vec<Row> {
    let mut rows = vec![Row {
        host: host.into(),
        attach: None,
        label: "  vms".into(),
    }];
    if vms.is_empty() {
        rows.push(Row {
            host: host.into(),
            attach: None,
            label: "    no vms".into(),
        });
    } else {
        rows.extend(vms.into_iter().map(|record| Row {
            host: host.into(),
            attach: Some(vm_attach(&record.id, args)),
            label: format!("    {} ({})  vm:{}", record.name, record.status, record.id),
        }));
    }
    rows
}

fn host_attach(host: &str, session_id: &str, args: &TuiArgs) -> PendingAttach {
    if args.local || args.socket.is_some() {
        PendingAttach {
            target: session_id.into(),
            socket: args.socket.clone(),
            config_path: None,
            host: None,
            local: args.local,
            state_dir: None,
        }
    } else {
        PendingAttach {
            target: format!("host:{host}:{session_id}"),
            socket: None,
            config_path: args.config.clone(),
            host: None,
            local: false,
            state_dir: None,
        }
    }
}

fn vm_attach(id: &str, args: &TuiArgs) -> PendingAttach {
    PendingAttach {
        target: format!("vm:{id}"),
        socket: None,
        config_path: args.config.clone(),
        host: None,
        local: false,
        state_dir: None,
    }
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

    fn vm_snapshot() -> vm::VmSnapshot {
        vm::VmSnapshot {
            schema_version: 1,
            observed_at_unix_ms: 1,
            state_root: "/state".into(),
            vms: vec![vm::VmRecord {
                schema_version: 1,
                id: "test".into(),
                name: "Test VM".into(),
                status: "running".into(),
                host: "local".into(),
                project: None,
                source_repo: None,
                profile_path: None,
                guest_workdir: None,
                packages: Vec::new(),
                ports: Vec::new(),
                repo_path: None,
                workdir: None,
                instance_dir: None,
                flake_path: None,
                microvm_config: None,
                runner_path: None,
                log_path: None,
                pid: Some(42),
                console_session: None,
                ssh_host: Some("127.0.0.1".into()),
                ssh_port: Some(2222),
                ssh_user: Some("root".into()),
                created_at_unix_ms: None,
                updated_at_unix_ms: None,
                started_at_unix_ms: None,
            }],
        }
    }

    fn args() -> TuiArgs {
        TuiArgs {
            socket: None,
            config: Some(PathBuf::from("config.local.toml")),
            host: None,
            local: false,
        }
    }

    #[test]
    fn rows_group_host_sessions_and_vms() {
        let rows = rows(&snapshot(), Some(&vm_snapshot()), None, &args());
        assert_eq!(rows[0].label, "local (ok)");
        assert!(rows.iter().any(|row| row.label == "  sessions"));
        assert!(rows.iter().any(|row| row.label.contains("work")));
        assert!(rows.iter().any(|row| row.label == "  vms"));
        assert!(rows.iter().any(|row| row.label.contains("Test VM")));
        assert!(
            rows.iter()
                .any(|row| row.label.contains("sessions error timeout"))
        );
    }

    #[test]
    fn rows_attach_with_typed_targets() {
        let rows = rows(&snapshot(), Some(&vm_snapshot()), None, &args());
        let host = rows
            .iter()
            .find_map(|row| {
                row.attach
                    .as_ref()
                    .filter(|attach| attach.target.starts_with("host:"))
            })
            .unwrap();
        assert_eq!(host.target, "host:local:$1");
        assert_eq!(host.config_path, Some(PathBuf::from("config.local.toml")));
        let vm = rows
            .iter()
            .find_map(|row| {
                row.attach
                    .as_ref()
                    .filter(|attach| attach.target == "vm:test")
            })
            .unwrap();
        assert_eq!(vm.config_path, Some(PathBuf::from("config.local.toml")));
    }

    #[test]
    fn local_socket_attach_preserves_socket_path() {
        let mut args = args();
        args.socket = Some("/tmp/tmux.sock".into());
        let attach = host_attach("local", "$1", &args);
        assert_eq!(attach.target, "$1");
        assert_eq!(attach.socket.as_deref(), Some("/tmp/tmux.sock"));
    }

    #[test]
    fn navigation_stays_in_bounds() {
        let mut app = App::new(
            SnapshotRequest {
                machines: Vec::new(),
                timeout_secs: 1,
                parallel: 1,
            },
            args(),
        );
        app.snapshot = snapshot();
        let vms = vm_snapshot();
        app.rows = rows(&app.snapshot, Some(&vms), None, &app.args);
        app.move_up();
        assert_eq!(app.selected, 0);
        for _ in 0..10 {
            app.move_down();
        }
        assert_eq!(app.selected, app.rows.len() - 1);
    }
}
