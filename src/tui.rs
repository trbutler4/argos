use crate::discovery::{RefreshJob, Update, observed_now};
use crate::sessions::{BrowseOptions, GuestMode, HostLocation, SessionTarget};
use crate::{Host, Snapshot, SnapshotRequest, vm};
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
    io::{self, Stdout},
    sync::mpsc::TryRecvError,
    time::Duration,
};

#[cfg(target_os = "linux")]
pub(crate) enum PickResult {
    Select(SessionTarget),
    Retry,
    Quit(u8),
}

#[cfg(target_os = "linux")]
pub(crate) fn pick_session(
    request: SnapshotRequest,
    options: &BrowseOptions,
    previous: Option<&SessionTarget>,
    error: Option<&str>,
    owner: &mut crate::terminal::TerminalOwner,
) -> io::Result<PickResult> {
    let mut app = App::new(request, options.clone());
    let mut notice = match error {
        Some(error) => {
            format!("Connection failed: {error}\nEnter choose | R retry last target | q quit")
        }
        None => "Origin-side session picker. Enter connect | q quit".into(),
    };
    if let Some(previous) = previous {
        notice.push_str(&format!(
            "\nEsc reconnects last Argos-selected target: {:?} (not native tmux switches)",
            previous.to_string()
        ));
    }
    app.notice = Some(notice);
    app.rebuild_rows();
    let mut terminal = TerminalGuard::enter()?;
    let mut job = Some(RefreshJob::start(
        app.request.clone(),
        app.args.state_dir.clone(),
    )?);
    let mut refresh_pending = false;
    loop {
        if let Some(code) = owner.poll_idle()? {
            return Ok(PickResult::Quit(code));
        }
        // Bounded updates per frame keep input responsive with many fast hosts.
        for _ in 0..16 {
            let Some(active) = &job else { break };
            match active.try_recv() {
                Ok(Update::Finished) => {
                    job.take().expect("active refresh").stop()?;
                    app.finish_refresh();
                    if refresh_pending {
                        refresh_pending = false;
                        app.begin_refresh();
                        job = Some(RefreshJob::start(
                            app.request.clone(),
                            app.args.state_dir.clone(),
                        )?);
                    }
                    break;
                }
                Ok(update) => app.apply(update),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    job.take().expect("active refresh").stop()?;
                    return Err(io::Error::other(
                        "discovery worker stopped without completing",
                    ));
                }
            }
        }
        terminal.terminal.draw(|frame| app.render(frame))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') => return Ok(PickResult::Quit(0)),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(PickResult::Quit(130));
                }
                KeyCode::Esc | KeyCode::Char('R') => {
                    return Ok(if previous.is_some() {
                        PickResult::Retry
                    } else {
                        PickResult::Quit(0)
                    });
                }
                _ => match app.handle_key(key) {
                    Some(Action::Attach(attach)) => return Ok(PickResult::Select(attach)),
                    Some(Action::Refresh) => {
                        if job.is_some() {
                            refresh_pending = true;
                        } else {
                            app.begin_refresh();
                            job = Some(RefreshJob::start(
                                app.request.clone(),
                                app.args.state_dir.clone(),
                            )?);
                        }
                    }
                    _ => {}
                },
            }
        }
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    _screen: ScreenGuard,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        let screen = ScreenGuard::enter()?;
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self {
            terminal,
            _screen: screen,
        })
    }
}

// Own modes before fallible backend initialization, so partial setup rolls back.
struct ScreenGuard;

impl ScreenGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(guard)
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Quit,
    Refresh,
    Attach(SessionTarget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    host: String,
    attach: Option<SessionTarget>,
    label: String,
}

struct App {
    request: SnapshotRequest,
    args: BrowseOptions,
    snapshot: Snapshot,
    vm_snapshot: Option<vm::VmSnapshot>,
    vm_error: Option<String>,
    rows: Vec<Row>,
    selected: usize,
    last_status: String,
    notice: Option<String>,
    loading: bool,
    preserve_selection: bool,
}

impl App {
    fn new(request: SnapshotRequest, args: BrowseOptions) -> Self {
        let hosts = request
            .machines
            .iter()
            .map(|target| Host {
                id: target.id.clone(),
                status: "loading",
                sessions: Vec::new(),
                error: None,
            })
            .collect();
        Self {
            request,
            args,
            snapshot: Snapshot {
                schema_version: 1,
                observed_at_unix_ms: 0,
                hosts,
            },
            vm_snapshot: None,
            vm_error: None,
            rows: Vec::new(),
            selected: 0,
            last_status: "loading".into(),
            notice: None,
            loading: true,
            preserve_selection: false,
        }
    }

    fn begin_refresh(&mut self) {
        self.loading = true;
        for host in &mut self.snapshot.hosts {
            host.status = "refreshing";
        }
        self.rebuild_rows();
    }

    fn finish_refresh(&mut self) {
        self.loading = false;
        self.snapshot.observed_at_unix_ms = observed_now();
        self.rebuild_rows();
    }

    fn apply(&mut self, update: Update) {
        match update {
            Update::Host(host) => {
                if let Some(slot) = self.snapshot.hosts.iter_mut().find(|old| old.id == host.id) {
                    *slot = host;
                }
            }
            Update::Vms(Ok(snapshot)) => {
                self.vm_snapshot = Some(snapshot);
                self.vm_error = None;
            }
            Update::Vms(Err(error)) => {
                self.vm_error = Some(error.to_string());
                self.vm_snapshot = None;
            }
            Update::Finished => unreachable!("job completion is handled by the picker"),
        }
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let selected_target = self
            .rows
            .get(self.selected)
            .and_then(|row| row.attach.as_ref())
            .cloned();
        self.rows = rows(
            &self.snapshot,
            self.vm_snapshot.as_ref(),
            self.vm_error.as_deref(),
            &self.args,
        );
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        if self.preserve_selection
            && let Some(index) = selected_target.and_then(|target| {
                self.rows
                    .iter()
                    .position(|row| row.attach.as_ref() == Some(&target))
            })
        {
            self.selected = index;
        } else if (!self.preserve_selection
            || self
                .rows
                .get(self.selected)
                .is_none_or(|row| row.attach.is_none()))
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
            "{} | {} hosts, {total_sessions} sessions, {total_vms} vms, {failed} errors",
            if self.loading {
                "refreshing (earlier rows may be stale)"
            } else {
                "READY"
            },
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
                self.preserve_selection = true;
                self.move_down();
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.preserve_selection = true;
                self.move_up();
                None
            }
            KeyCode::Char('r') => {
                self.preserve_selection = self
                    .rows
                    .get(self.selected)
                    .is_some_and(|row| row.attach.is_some());
                Some(Action::Refresh)
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
                Constraint::Length(6),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(area);

        let title = Paragraph::new(if let Some(notice) = &self.notice {
            std::iter::once(Line::from("Argos sessions"))
                .chain(notice.lines().map(Line::from))
                .collect()
        } else {
            vec![Line::from(vec![
                Span::styled(
                    "Argos",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" sessions and VMs"),
            ])]
        })
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
                    .map(|attach| format!(" selected {attach}"))
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
    args: &BrowseOptions,
) -> Vec<Row> {
    let empty_vm_label = if vm_error.is_some() {
        "    VM state unavailable"
    } else if vm_snapshot.is_some() {
        "    no vms"
    } else {
        "    loading vms"
    };
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
            empty_vm_label,
        ));
    }
    for (host, records) in vms_by_host {
        if rendered_hosts.insert(host.clone()) {
            rows.extend(vm_only_host_rows(&host, records));
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

fn host_rows(
    host: &Host,
    vms: Vec<&vm::VmRecord>,
    args: &BrowseOptions,
    empty_vm_label: &str,
) -> Vec<Row> {
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
                label: if host.status == "loading" {
                    "    loading sessions"
                } else {
                    "    no tmux sessions"
                }
                .into(),
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
    rows.extend(vm_section_rows(&host.id, vms, empty_vm_label));
    rows
}

fn vm_only_host_rows(host: &str, vms: Vec<&vm::VmRecord>) -> Vec<Row> {
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
    rows.extend(vm_section_rows(host, vms, "    no vms"));
    rows
}

fn vm_section_rows(host: &str, vms: Vec<&vm::VmRecord>, empty_label: &str) -> Vec<Row> {
    let mut rows = vec![Row {
        host: host.into(),
        attach: None,
        label: "  vms".into(),
    }];
    if vms.is_empty() {
        rows.push(Row {
            host: host.into(),
            attach: None,
            label: empty_label.into(),
        });
    } else {
        rows.extend(vms.into_iter().map(|record| Row {
            host: host.into(),
            attach: Some(vm_attach(&record.id)),
            label: format!("    {} ({})  vm:{}", record.name, record.status, record.id),
        }));
    }
    rows
}

fn host_attach(host: &str, session_id: &str, args: &BrowseOptions) -> SessionTarget {
    SessionTarget::HostTmux {
        session: session_id.into(),
        location: if args.local || args.socket.is_some() {
            HostLocation::Local {
                socket: args.socket.clone(),
            }
        } else {
            HostLocation::Inventory {
                machine: Some(host.into()),
            }
        },
    }
}

fn vm_attach(id: &str) -> SessionTarget {
    SessionTarget::Vm {
        id: id.into(),
        mode: GuestMode::Tmux,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{discovery::HostError, tmux};
    use std::{path::PathBuf, time::UNIX_EPOCH};

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
                guest_modules: Vec::new(),
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
                resources: vm::VmResources::default(),
            }],
        }
    }

    fn args() -> BrowseOptions {
        BrowseOptions {
            socket: None,
            config_path: Some(PathBuf::from("config.local.toml")),
            host: None,
            local: false,
            state_dir: None,
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
                    .filter(|attach| matches!(attach, SessionTarget::HostTmux { .. }))
            })
            .unwrap();
        assert_eq!(
            host,
            &SessionTarget::HostTmux {
                session: "$1".into(),
                location: HostLocation::Inventory {
                    machine: Some("local".into())
                }
            }
        );
        let vm = rows
            .iter()
            .find_map(|row| {
                row.attach
                    .as_ref()
                    .filter(|attach| matches!(attach, SessionTarget::Vm { .. }))
            })
            .unwrap();
        assert_eq!(
            vm,
            &SessionTarget::Vm {
                id: "test".into(),
                mode: GuestMode::Tmux
            }
        );
    }

    #[test]
    fn local_socket_attach_preserves_socket_path() {
        let mut args = args();
        args.socket = Some("/tmp/tmux.sock".into());
        let attach = host_attach("local", "$1", &args);
        assert_eq!(
            attach,
            SessionTarget::HostTmux {
                session: "$1".into(),
                location: HostLocation::Local {
                    socket: Some("/tmp/tmux.sock".into())
                }
            }
        );
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
