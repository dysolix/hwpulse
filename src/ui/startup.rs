//! Startup apps page: what starts at sign-in, with a switch per entry like Task
//! Manager. Listed off the UI thread (shortcuts are resolved through COM) and again
//! every few seconds while open, so changes made elsewhere show up.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{self, ScrollArea, Ui};

use super::table::{self, Job, Notice, Sort};
use super::theme;
use crate::shell;
use crate::startup::{self, Entry};
use crate::tasks;

const REFRESH: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Column {
    Name,
    Publisher,
    Status,
    Disabled,
    Source,
    Command,
}

impl table::Column for Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Publisher => "Publisher",
            Column::Status => "Status",
            Column::Disabled => "Disabled",
            Column::Source => "Startup type",
            Column::Command => "Command",
        }
    }

    fn width(self) -> f32 {
        match self {
            Column::Name => 180.0,
            Column::Publisher => 170.0,
            Column::Status => 80.0,
            Column::Disabled => 100.0,
            Column::Source => 170.0,
            Column::Command => 220.0,
        }
    }

    fn flexible(self) -> bool {
        matches!(self, Column::Name | Column::Command)
    }
}

const COLUMNS: [Column; 6] =
    [Column::Name, Column::Publisher, Column::Status, Column::Disabled, Column::Source, Column::Command];
const HIDE_ORDER: [Column; 4] = [Column::Command, Column::Disabled, Column::Source, Column::Publisher];

pub struct State {
    entries: Option<Vec<Entry>>,
    loading: Option<Job<Vec<Entry>>>,
    loaded: Option<Instant>,
    query: String,
    sort: Sort<Column>,
    /// Name and source of the selected entry.
    selected: Option<(String, startup::Source)>,
    notice: Option<Notice>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            entries: None,
            loading: None,
            loaded: None,
            query: String::new(),
            // Enabled first, then by name.
            sort: Sort { column: Column::Status, descending: false },
            selected: None,
            notice: None,
        }
    }
}

pub fn page(ui: &mut Ui, state: &mut State) {
    if let Some(job) = &state.loading
        && let Some(entries) = job.take()
    {
        state.entries = Some(entries);
        state.loaded = Some(Instant::now());
        state.loading = None;
    }
    if state.loading.is_none() && state.loaded.is_none_or(|t| t.elapsed() >= REFRESH) {
        state.loading = Some(Job::start(ui.ctx(), startup::list));
    }
    ui.ctx().request_repaint_after(REFRESH);
    let Some(entries) = state.entries.clone() else {
        super::widgets::waiting(ui, "Listing startup apps…", "Reading the Run keys and Startup folders");
        return;
    };

    let query = state.query.trim().to_lowercase();
    let mut visible: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            query.is_empty()
                || e.name.to_lowercase().contains(&query)
                || e.command.to_lowercase().contains(&query)
                || e.publisher.as_ref().is_some_and(|p| p.to_lowercase().contains(&query))
        })
        .collect();
    let sort = state.sort;
    visible.sort_by(|a, b| {
        sort.apply(compare(sort.column, a, b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let selected =
        state.selected.as_ref().and_then(|(n, s)| entries.iter().find(|e| &e.name == n && e.source == *s)).cloned();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        table::search(ui, &mut state.query, "Search name, publisher or command", 280.0);
        let on = entries.iter().filter(|e| e.enabled).count();
        table::count(ui, format!("{} apps · {on} on", entries.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, on) = match &selected {
                Some(e) if e.enabled => ("Disable", false),
                _ => ("Enable", true),
            };
            if table::button(ui, label, selected.is_some(), false).clicked()
                && let Some(e) = &selected
            {
                switch(state, e, on);
            }
            table::admin_badge(ui);
        });
    });
    ui.add_space(-2.0);
    table::notice(ui, &mut state.notice, "startup");

    table::card(ui, |ui| {
        let columns = table::layout(&COLUMNS, &HIDE_ORDER, ui.available_width() - 12.0);
        table::header(ui, &mut state.sort, &columns);
        ScrollArea::vertical().id_salt("startup").auto_shrink(false).show_rows(
            ui,
            table::ROW,
            visible.len(),
            |ui, range| {
                for entry in &visible[range] {
                    row(ui, state, entry, &columns);
                }
            },
        );
    });
}

fn switch(state: &mut State, entry: &Entry, on: bool) {
    match startup::set_enabled(entry, on) {
        Ok(()) => {
            state.notice = None;
            state.loaded = None;
            if let Some(e) = state
                .entries
                .as_mut()
                .and_then(|list| list.iter_mut().find(|e| e.name == entry.name && e.source == entry.source))
            {
                e.enabled = on;
            }
        }
        Err(tasks::Error::AccessDenied) => {
            state.notice = Some(Notice::denied(format!(
                "Access denied: {} starts for all users, which only an administrator can change.",
                entry.name
            )))
        }
        Err(e) => state.notice = Some(Notice::info(format!("Could not change {}: {e:?}", entry.name))),
    }
}

fn compare(column: Column, a: &Entry, b: &Entry) -> Ordering {
    let text = |x: &Option<String>, y: &Option<String>| {
        x.as_deref().unwrap_or("").to_lowercase().cmp(&y.as_deref().unwrap_or("").to_lowercase())
    };
    match column {
        Column::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        Column::Publisher => text(&a.publisher, &b.publisher),
        Column::Status => b.enabled.cmp(&a.enabled),
        Column::Disabled => a.disabled_at.cmp(&b.disabled_at),
        Column::Source => a.source.cmp(&b.source),
        Column::Command => a.command.to_lowercase().cmp(&b.command.to_lowercase()),
    }
}

fn row(ui: &mut Ui, state: &mut State, entry: &Entry, columns: &[(Column, f32)]) {
    let key = (entry.name.clone(), entry.source);
    let (rect, response, painter) = table::row(ui, state.selected.as_ref() == Some(&key));
    if response.clicked() || response.secondary_clicked() {
        state.selected = Some(key.clone());
    }
    response.context_menu(|ui| {
        state.selected = Some(key.clone());
        let mut close = false;
        if ui.button(if entry.enabled { "Disable" } else { "Enable" }).clicked() {
            switch(state, entry, !entry.enabled);
            close = true;
        }
        ui.separator();
        if let Some(path) = &entry.path {
            if ui.button("Open file location").clicked() {
                shell::open_location(path);
                close = true;
            }
            if ui.button("Properties").clicked() {
                shell::properties(path);
                close = true;
            }
        }
        if ui.button("Search online").clicked() {
            shell::search_online(&entry.name);
            close = true;
        }
        if ui.button("Copy command").clicked() {
            ui.ctx().copy_text(entry.command.clone());
            close = true;
        }
        if close {
            ui.close();
        }
    });

    for (column, cell) in table::cells(rect, columns) {
        let dim = |s: String| (s, theme::TEXT_DIM);
        let (text, color) = match column {
            Column::Name => {
                table::name(&painter, cell, &entry.name, 0, None, false);
                continue;
            }
            Column::Publisher => dim(entry.publisher.clone().unwrap_or_default()),
            Column::Status if entry.enabled => ("Enabled".into(), theme::LIVE),
            Column::Status => dim("Disabled".into()),
            Column::Disabled => match entry.disabled_at.map(startup::days_since) {
                Some(0) => dim("Today".into()),
                Some(1) => dim("Yesterday".into()),
                Some(d) => dim(format!("{d} days ago")),
                None => (String::new(), theme::TEXT_FAINT),
            },
            Column::Source => dim(entry.source.label().into()),
            Column::Command => dim(entry.command.clone()),
        };
        table::text(&painter, cell, column, &text, color);
    }
}
