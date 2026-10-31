//! Services page: Task Manager's Services tab with start type, refreshed every two
//! seconds while open. Listing and controlling run off the UI thread; most services
//! need admin rights to start or stop.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{self, ScrollArea, Ui};

use super::table::{self, Job, Notice, Sort};
use super::{Page, theme};
use crate::services::{self, Control, Service, Status};
use crate::shell;
use crate::tasks;

const REFRESH: Duration = Duration::from_secs(2);

type Running = Job<Result<(), tasks::Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Column {
    Name,
    Pid,
    Description,
    Status,
    Start,
    Group,
}

impl table::Column for Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Pid => "PID",
            Column::Description => "Description",
            Column::Status => "Status",
            Column::Start => "Startup type",
            Column::Group => "Group",
        }
    }

    fn width(self) -> f32 {
        match self {
            Column::Name => 170.0,
            Column::Pid => 64.0,
            Column::Description => 220.0,
            Column::Status => 80.0,
            Column::Start => 140.0,
            Column::Group => 150.0,
        }
    }

    fn numeric(self) -> bool {
        self == Column::Pid
    }

    fn flexible(self) -> bool {
        matches!(self, Column::Name | Column::Description)
    }
}

const COLUMNS: [Column; 6] =
    [Column::Name, Column::Pid, Column::Description, Column::Status, Column::Start, Column::Group];
const HIDE_ORDER: [Column; 3] = [Column::Group, Column::Start, Column::Pid];

pub struct State {
    services: Option<Result<Vec<Service>, String>>,
    loading: Option<Job<Result<Vec<Service>, String>>>,
    loaded: Option<Instant>,
    query: String,
    sort: Sort<Column>,
    selected: Option<String>,
    notice: Option<Notice>,
    /// The service being started or stopped, with what is done to it.
    running: Option<(String, Control, Running)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            services: None,
            loading: None,
            loaded: None,
            query: String::new(),
            sort: Sort { column: Column::Name, descending: false },
            selected: None,
            notice: None,
            running: None,
        }
    }
}

impl State {
    /// Shows only the services in process `pid` ("Go to services").
    pub fn show_pid(&mut self, pid: u32) {
        self.query = pid.to_string();
        self.selected = None;
    }
}

pub fn page(ui: &mut Ui, state: &mut State, nav: &mut Option<Page>) {
    refresh(ui.ctx(), state);
    let Some(services) = &state.services else {
        super::widgets::waiting(ui, "Listing services…", "Asking the Service Control Manager");
        return;
    };
    let services = match services {
        Ok(s) => s.clone(),
        Err(e) => {
            super::widgets::waiting(ui, "Could not list services", e);
            return;
        }
    };

    let query = state.query.trim().to_lowercase();
    let mut visible: Vec<&Service> = services
        .iter()
        .filter(|s| {
            query.is_empty()
                || s.name.to_lowercase().contains(&query)
                || s.display.to_lowercase().contains(&query)
                || s.pid.to_string() == query
                || s.group.as_ref().is_some_and(|g| g.to_lowercase().contains(&query))
        })
        .collect();
    let sort = state.sort;
    visible.sort_by(|a, b| sort.apply(compare(sort.column, a, b)).then_with(|| a.name.cmp(&b.name)));
    let selected = state.selected.as_ref().and_then(|n| services.iter().find(|s| &s.name == n)).cloned();

    toolbar(ui, state, services.len(), visible.len(), selected.as_ref());
    table::notice(ui, &mut state.notice, "services");

    table::card(ui, |ui| {
        let columns = table::layout(&COLUMNS, &HIDE_ORDER, ui.available_width() - 12.0);
        table::header(ui, &mut state.sort, &columns);
        ScrollArea::vertical().id_salt("services").auto_shrink(false).show_rows(
            ui,
            table::ROW,
            visible.len(),
            |ui, range| {
                for service in &visible[range] {
                    row(ui, state, service, &columns, nav);
                }
            },
        );
    });
}

fn refresh(ctx: &egui::Context, state: &mut State) {
    if let Some(job) = &state.loading
        && let Some(result) = job.take()
    {
        state.services = Some(result);
        state.loaded = Some(Instant::now());
        state.loading = None;
    }
    if let Some((name, control, job)) = &state.running
        && let Some(result) = job.take()
    {
        let verb = match control {
            Control::Start => "start",
            Control::Stop => "stop",
            Control::Restart => "restart",
        };
        state.notice = match result {
            Ok(()) => None,
            Err(tasks::Error::AccessDenied) => Some(Notice::denied(format!(
                "Access denied: could not {verb} {name}. Most services need administrator rights."
            ))),
            Err(tasks::Error::Gone) => None,
            Err(tasks::Error::Other(e)) => Some(Notice::info(format!("Could not {verb} {name}: {e}"))),
        };
        state.running = None;
        state.loaded = None; // show the new state right away
    }
    if state.loading.is_none() && state.loaded.is_none_or(|t| t.elapsed() >= REFRESH) {
        state.loading = Some(Job::start(ctx, services::list));
    }
    ctx.request_repaint_after(REFRESH);
}

fn compare(column: Column, a: &Service, b: &Service) -> Ordering {
    match column {
        Column::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        Column::Pid => a.pid.cmp(&b.pid),
        Column::Description => a.display.to_lowercase().cmp(&b.display.to_lowercase()),
        Column::Status => a.status.cmp(&b.status),
        Column::Start => a.start.cmp(&b.start),
        Column::Group => a.group.cmp(&b.group),
    }
}

fn toolbar(ui: &mut Ui, state: &mut State, total: usize, shown: usize, selected: Option<&Service>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        table::search(ui, &mut state.query, "Search name, description, PID or group", 280.0);
        table::count(ui, if shown == total { format!("{total} services") } else { format!("{shown} of {total}") });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if table::button(ui, "Open Services", true, false).clicked() {
                shell::open("services.msc");
            }
            let busy = state.running.is_some();
            let status = selected.map(|s| s.status);
            for (label, control, enabled) in [
                ("Restart", Control::Restart, status == Some(Status::Running)),
                ("Stop", Control::Stop, status == Some(Status::Running)),
                ("Start", Control::Start, status == Some(Status::Stopped)),
            ] {
                if table::button(ui, label, enabled && !busy, false).clicked()
                    && let Some(s) = selected
                {
                    start(ui.ctx(), state, s, control);
                }
            }
            table::admin_badge(ui);
        });
    });
    ui.add_space(-2.0);
}

fn start(ctx: &egui::Context, state: &mut State, service: &Service, control: Control) {
    let name = service.name.clone();
    let job = Job::start(ctx, {
        let name = name.clone();
        move || services::control(&name, control)
    });
    let verb = match control {
        Control::Start => "Starting",
        Control::Stop => "Stopping",
        Control::Restart => "Restarting",
    };
    state.notice = Some(Notice::info(format!("{verb} {}…", service.display)));
    state.running = Some((name, control, job));
}

fn row(ui: &mut Ui, state: &mut State, service: &Service, columns: &[(Column, f32)], nav: &mut Option<Page>) {
    let selected = state.selected.as_deref() == Some(&service.name);
    let (rect, response, painter) = table::row(ui, selected);
    if response.clicked() || response.secondary_clicked() {
        state.selected = Some(service.name.clone());
    }
    response.context_menu(|ui| {
        state.selected = Some(service.name.clone());
        let busy = state.running.is_some();
        let mut close = false;
        for (label, control, enabled) in [
            ("Start", Control::Start, service.status == Status::Stopped),
            ("Stop", Control::Stop, service.status == Status::Running),
            ("Restart", Control::Restart, service.status == Status::Running),
        ] {
            if ui.add_enabled(enabled && !busy, egui::Button::new(label)).clicked() {
                start(ui.ctx(), state, service, control);
                close = true;
            }
        }
        ui.separator();
        if ui.button("Open Services").clicked() {
            shell::open("services.msc");
            close = true;
        }
        if ui.button("Search online").clicked() {
            shell::search_online(&format!("{} {}", service.name, service.display));
            close = true;
        }
        if service.pid != 0 && ui.button("Go to details").clicked() {
            *nav = Some(Page::Process(service.pid));
            close = true;
        }
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(service.name.clone());
            close = true;
        }
        if close {
            ui.close();
        }
    });

    for (column, cell) in table::cells(rect, columns) {
        let dim = |s: &str| (s.to_owned(), theme::TEXT_DIM);
        let (text, color) = match column {
            Column::Name => {
                table::name(&painter, cell, &service.name, 0, None, false);
                continue;
            }
            Column::Pid if service.pid == 0 => (String::new(), theme::TEXT_FAINT),
            Column::Pid => dim(&service.pid.to_string()),
            Column::Description => dim(&service.display),
            Column::Status => (
                service.status.label().to_owned(),
                match service.status {
                    Status::Running => theme::LIVE,
                    Status::Stopped => theme::TEXT_DIM,
                    _ => theme::ACCENT,
                },
            ),
            Column::Start => match service.start {
                Some(s) => dim(s.label()),
                None => ("—".into(), theme::TEXT_FAINT),
            },
            Column::Group => dim(service.group.as_deref().unwrap_or("")),
        };
        table::text(&painter, cell, column, &text, color);
    }
}
