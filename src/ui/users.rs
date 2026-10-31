//! Users page: one row per signed-in user with the summed use of their processes,
//! expandable to those processes, like Task Manager's Users tab. Uses the same process
//! list as the Processes page.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui::{self, ScrollArea, Ui};

use super::table::{self, Job, Notice, Sort};
use super::{Page, theme, widgets};
use crate::poller::Model;
use crate::sensors::{ProcessRow, Unit};
use crate::shell;
use crate::tasks;
use crate::users::{self, Session};

const REFRESH: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Column {
    User,
    Status,
    Processes,
    Cpu,
    Memory,
    Disk,
    Gpu,
}

impl table::Column for Column {
    fn title(self) -> &'static str {
        match self {
            Column::User => "User",
            Column::Status => "Status",
            Column::Processes => "Processes",
            Column::Cpu => "CPU",
            Column::Memory => "Memory",
            Column::Disk => "Disk",
            Column::Gpu => "GPU",
        }
    }

    fn width(self) -> f32 {
        match self {
            Column::User => 220.0,
            Column::Status => 100.0,
            Column::Processes => 80.0,
            Column::Cpu | Column::Gpu => 64.0,
            Column::Memory | Column::Disk => 90.0,
        }
    }

    fn numeric(self) -> bool {
        !matches!(self, Column::User | Column::Status)
    }

    fn flexible(self) -> bool {
        self == Column::User
    }
}

const COLUMNS: [Column; 7] =
    [Column::User, Column::Status, Column::Processes, Column::Cpu, Column::Memory, Column::Disk, Column::Gpu];
const HIDE_ORDER: [Column; 3] = [Column::Processes, Column::Gpu, Column::Status];

#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Disconnect,
    SignOut,
}

pub struct State {
    sessions: Vec<Session>,
    loading: Option<Job<Vec<Session>>>,
    loaded: Option<Instant>,
    sort: Sort<Column>,
    expanded: HashSet<u32>,
    selected: Option<u32>,
    pending: Option<(Ask, Session)>,
    notice: Option<Notice>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            loading: None,
            loaded: None,
            sort: Sort { column: Column::Cpu, descending: true },
            expanded: HashSet::new(),
            selected: None,
            pending: None,
            notice: None,
        }
    }
}

/// A user with the sums of their processes.
struct Line<'a> {
    session: &'a Session,
    processes: Vec<&'a ProcessRow>,
    cpu: f64,
    memory: u64,
    disk: f64,
    gpu: f64,
}

pub fn page(ui: &mut Ui, model: &Model, state: &mut State, nav: &mut Option<Page>) {
    if let Some(job) = &state.loading
        && let Some(sessions) = job.take()
    {
        state.sessions = sessions;
        state.loaded = Some(Instant::now());
        state.loading = None;
    }
    if state.loading.is_none() && state.loaded.is_none_or(|t| t.elapsed() >= REFRESH) {
        state.loading = Some(Job::start(ui.ctx(), users::sessions));
    }
    ui.ctx().request_repaint_after(REFRESH);
    let (Some(rows), Some(_)) = (&model.processes, state.loaded) else {
        widgets::waiting(ui, "Listing users…", "Reading sessions and the processes of each user");
        return;
    };

    let sessions = state.sessions.clone();
    let mut lines: Vec<Line> = sessions
        .iter()
        .map(|session| {
            let mut processes: Vec<&ProcessRow> = rows.iter().filter(|r| r.process.session_id == session.id).collect();
            processes.sort_by(|a, b| b.process.cpu.total_cmp(&a.process.cpu));
            Line {
                session,
                cpu: processes.iter().map(|r| r.process.cpu).sum(),
                memory: processes.iter().map(|r| r.process.private_bytes).sum(),
                disk: processes.iter().map(|r| r.process.io_bytes_per_sec).sum(),
                gpu: processes.iter().map(|r| r.gpu.load).sum::<f64>().min(100.0),
                processes,
            }
        })
        .collect();
    let sort = state.sort;
    lines.sort_by(|a, b| sort.apply(compare(sort.column, a, b)).then_with(|| a.session.user.cmp(&b.session.user)));
    let selected = state.selected.and_then(|id| sessions.iter().find(|s| s.id == id)).cloned();
    let own = users::current();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        table::count(ui, format!("{} signed in", sessions.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if table::button(ui, "Manage accounts", true, false).clicked() {
                shell::open("ms-settings:otherusers");
            }
            let any = selected.is_some();
            if table::button(ui, "Sign out", any, true).clicked() {
                state.pending = selected.clone().map(|s| (Ask::SignOut, s));
            }
            let connected = selected.as_ref().is_some_and(|s| s.status == users::Status::Active);
            if table::button(ui, "Disconnect", connected, false).clicked() {
                state.pending = selected.clone().map(|s| (Ask::Disconnect, s));
            }
            table::admin_badge(ui);
        });
    });
    ui.add_space(-2.0);
    table::notice(ui, &mut state.notice, "users");

    table::card(ui, |ui| {
        let columns = table::layout(&COLUMNS, &HIDE_ORDER, ui.available_width() - 12.0);
        table::header(ui, &mut state.sort, &columns);
        ScrollArea::vertical().id_salt("users").auto_shrink(false).show(ui, |ui| {
            for line in &lines {
                user_row(ui, state, line, &columns, own);
                if state.expanded.contains(&line.session.id) {
                    for row in &line.processes {
                        process_row(ui, row, &columns, nav);
                    }
                }
            }
        });
    });

    confirm(ui, state, own);
}

fn compare(column: Column, a: &Line, b: &Line) -> Ordering {
    match column {
        Column::User => a.session.user.to_lowercase().cmp(&b.session.user.to_lowercase()),
        Column::Status => a.session.status.cmp(&b.session.status),
        Column::Processes => a.processes.len().cmp(&b.processes.len()),
        Column::Cpu => a.cpu.total_cmp(&b.cpu),
        Column::Memory => a.memory.cmp(&b.memory),
        Column::Disk => a.disk.total_cmp(&b.disk),
        Column::Gpu => a.gpu.total_cmp(&b.gpu),
    }
}

fn user_row(ui: &mut Ui, state: &mut State, line: &Line, columns: &[(Column, f32)], own: u32) {
    let id = line.session.id;
    let (rect, response, painter) = table::row(ui, state.selected == Some(id));
    if response.clicked() || response.secondary_clicked() {
        state.selected = Some(id);
    }
    if (response.double_clicked() || response.clicked() && table::chevron_hit(ui, rect, 0))
        && !state.expanded.remove(&id)
    {
        state.expanded.insert(id);
    }
    response.context_menu(|ui| {
        state.selected = Some(id);
        let mut close = false;
        let label = if state.expanded.contains(&id) { "Collapse" } else { "Expand" };
        if ui.button(label).clicked() {
            if !state.expanded.remove(&id) {
                state.expanded.insert(id);
            }
            close = true;
        }
        if ui.add_enabled(line.session.status == users::Status::Active, egui::Button::new("Disconnect")).clicked() {
            state.pending = Some((Ask::Disconnect, line.session.clone()));
            close = true;
        }
        if ui.button("Sign out").clicked() {
            state.pending = Some((Ask::SignOut, line.session.clone()));
            close = true;
        }
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(line.session.user.clone());
            close = true;
        }
        if close {
            ui.close();
        }
    });

    for (column, cell) in table::cells(rect, columns) {
        let (text, color) = match column {
            Column::User => {
                let mut name = line.session.user.clone();
                if id == own {
                    name += "  (you)";
                }
                table::name(&painter, cell, &name, 0, Some(state.expanded.contains(&id)), true);
                continue;
            }
            Column::Status => (line.session.status.label().to_owned(), theme::TEXT_DIM),
            Column::Processes => (line.processes.len().to_string(), theme::TEXT_DIM),
            Column::Cpu => (format!("{:.1}%", line.cpu), theme::accent(Unit::Percent, line.cpu)),
            Column::Memory => (widgets::format_value(line.memory as f64 / 1048576.0, Unit::Mebibytes), theme::TEXT),
            Column::Disk => (widgets::format_value(line.disk, Unit::BytesPerSec), theme::TEXT),
            Column::Gpu => (format!("{:.1}%", line.gpu), theme::TEXT_DIM),
        };
        table::text(&painter, cell, column, &text, color);
    }
}

fn process_row(ui: &mut Ui, row: &ProcessRow, columns: &[(Column, f32)], nav: &mut Option<Page>) {
    let (rect, response, painter) = table::row(ui, false);
    let p = &row.process;
    if response.double_clicked() {
        *nav = Some(Page::Process(p.pid));
    }
    response.context_menu(|ui| {
        if ui.button("Go to details").clicked() {
            *nav = Some(Page::Process(p.pid));
            ui.close();
        }
    });
    for (column, cell) in table::cells(rect, columns) {
        let faint = (String::from("0"), theme::TEXT_FAINT);
        let (text, color) = match column {
            Column::User => {
                let name = match &row.window {
                    Some(w) => format!("{}  ·  {}", p.name, w.title),
                    None => p.name.clone(),
                };
                table::name(&painter, cell, &name, 1, None, false);
                continue;
            }
            Column::Status if p.suspended => ("Suspended".into(), theme::ACCENT),
            Column::Status | Column::Processes => continue,
            Column::Cpu if p.cpu < 0.05 => faint,
            Column::Cpu => (format!("{:.1}%", p.cpu), theme::accent(Unit::Percent, p.cpu * 4.0)),
            Column::Memory => (widgets::format_value(p.private_bytes as f64 / 1048576.0, Unit::Mebibytes), theme::TEXT),
            Column::Disk if p.io_bytes_per_sec < 512.0 => faint,
            Column::Disk => (widgets::format_value(p.io_bytes_per_sec, Unit::BytesPerSec), theme::TEXT),
            Column::Gpu if row.gpu.load < 0.05 => faint,
            Column::Gpu => (format!("{:.1}%", row.gpu.load), theme::TEXT_DIM),
        };
        table::text(&painter, cell, column, &text, color);
    }
}

fn confirm(ui: &mut Ui, state: &mut State, own: u32) {
    let Some((ask, session)) = &state.pending else { return };
    let you = session.id == own;
    let (title, note, button) = match (ask, you) {
        (Ask::Disconnect, true) => (
            "Disconnect yourself?".to_owned(),
            "Your apps keep running; Windows shows the sign-in screen.",
            "Disconnect",
        ),
        (Ask::Disconnect, false) => (
            format!("Disconnect {}?", session.user),
            "Their apps keep running; they can sign in again to pick up where they left off.",
            "Disconnect",
        ),
        (Ask::SignOut, true) => ("Sign out of Windows?".to_owned(), "Unsaved work in your apps is lost.", "Sign out"),
        (Ask::SignOut, false) => {
            (format!("Sign out {}?", session.user), "Unsaved work in their apps is lost.", "Sign out")
        }
    };
    let Some(go) = table::confirm(ui, "confirm_user", &title, note, None, button) else { return };
    let (ask, session) = state.pending.take().expect("checked above");
    if !go {
        return;
    }
    let result = match ask {
        Ask::Disconnect => users::disconnect(session.id),
        Ask::SignOut => users::sign_out(session.id),
    };
    state.loaded = None;
    state.notice = match result {
        Ok(()) | Err(tasks::Error::Gone) => None,
        Err(tasks::Error::AccessDenied) => {
            Some(Notice::denied(format!("Access denied: only an administrator can do this for {}.", session.user)))
        }
        Err(tasks::Error::Other(e)) => Some(Notice::info(format!("Could not do this for {}: {e}", session.user))),
    };
}
