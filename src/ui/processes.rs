//! Processes page: every running process in a sortable, searchable table, Task
//! Manager's Processes and Details tabs in one. Only the visible rows are laid out and
//! drawn, so hundreds of processes cost no more than a screenful. Rows can be grouped
//! per executable and sorted into Apps, Background and Windows processes; columns can
//! be picked. Selected processes can be ended (alone or with everything they started),
//! suspended, resumed, reprioritized, pinned to processors, put into efficiency mode
//! and dumped. Without admin rights that fails for elevated and SYSTEM processes, and
//! the page offers to restart as admin.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui::{self, Key, ScrollArea, Ui};

use super::table::{self, Column as _, Job, Notice, Sort};
use super::{Page, theme, widgets};
use crate::poller::Model;
use crate::sensors::{ProcessRow, Unit};
use crate::settings::Settings;
use crate::shell;
use crate::tasks::{self, Action, Priority, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Column {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    CpuTime,
    Memory,
    WorkingSet,
    PeakWorkingSet,
    Commit,
    PageFaults,
    Disk,
    Gpu,
    GpuMemory,
    Threads,
    Handles,
    Priority,
    Session,
    ParentPid,
    Architecture,
    Elevated,
    Virtualization,
    Description,
    CommandLine,
}

impl Column {
    const ALL: [Column; 24] = [
        Column::Name,
        Column::Pid,
        Column::Status,
        Column::User,
        Column::Cpu,
        Column::CpuTime,
        Column::Memory,
        Column::WorkingSet,
        Column::PeakWorkingSet,
        Column::Commit,
        Column::PageFaults,
        Column::Disk,
        Column::Gpu,
        Column::GpuMemory,
        Column::Threads,
        Column::Handles,
        Column::Priority,
        Column::Session,
        Column::ParentPid,
        Column::Architecture,
        Column::Elevated,
        Column::Virtualization,
        Column::Description,
        Column::CommandLine,
    ];
    const DEFAULT: [Column; 12] = [
        Column::Name,
        Column::Pid,
        Column::Status,
        Column::User,
        Column::Cpu,
        Column::Memory,
        Column::Disk,
        Column::Gpu,
        Column::Threads,
        Column::Handles,
        Column::Architecture,
        Column::Description,
    ];
    /// Dropped first when the window gets narrow. Name, PID, CPU and memory always stay.
    const HIDE_ORDER: [Column; 20] = [
        Column::CommandLine,
        Column::Description,
        Column::Virtualization,
        Column::Elevated,
        Column::ParentPid,
        Column::Session,
        Column::Priority,
        Column::Architecture,
        Column::Handles,
        Column::Threads,
        Column::PageFaults,
        Column::PeakWorkingSet,
        Column::WorkingSet,
        Column::Commit,
        Column::CpuTime,
        Column::GpuMemory,
        Column::User,
        Column::Status,
        Column::Gpu,
        Column::Disk,
    ];

    /// Stable name in the settings file.
    fn key(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Pid => "pid",
            Column::Status => "status",
            Column::User => "user",
            Column::Cpu => "cpu",
            Column::CpuTime => "cpu_time",
            Column::Memory => "memory",
            Column::WorkingSet => "working_set",
            Column::PeakWorkingSet => "peak_working_set",
            Column::Commit => "commit",
            Column::PageFaults => "page_faults",
            Column::Disk => "disk",
            Column::Gpu => "gpu",
            Column::GpuMemory => "gpu_memory",
            Column::Threads => "threads",
            Column::Handles => "handles",
            Column::Priority => "priority",
            Column::Session => "session",
            Column::ParentPid => "parent_pid",
            Column::Architecture => "architecture",
            Column::Elevated => "elevated",
            Column::Virtualization => "virtualization",
            Column::Description => "description",
            Column::CommandLine => "command_line",
        }
    }

    /// What the column shows, for the column picker.
    fn hint(self) -> &'static str {
        match self {
            Column::Memory => "Private working set: RAM only this process uses",
            Column::WorkingSet => "All RAM in use, including memory shared with others",
            Column::Commit => "Memory reserved for it, in RAM or the page file",
            Column::Gpu => "Load of its busiest GPU engine",
            Column::GpuMemory => "Dedicated GPU memory",
            Column::Virtualization => "UAC file and registry virtualization",
            _ => "",
        }
    }

    /// Shown (and sorted) with a group's totals.
    fn summed(self) -> bool {
        matches!(
            self,
            Column::Cpu
                | Column::CpuTime
                | Column::Memory
                | Column::WorkingSet
                | Column::PeakWorkingSet
                | Column::Commit
                | Column::PageFaults
                | Column::Disk
                | Column::Gpu
                | Column::GpuMemory
                | Column::Threads
                | Column::Handles
                | Column::Status
                | Column::Name
        )
    }
}

impl table::Column for Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Pid => "PID",
            Column::Status => "Status",
            Column::User => "User",
            Column::Cpu => "CPU",
            Column::CpuTime => "CPU time",
            Column::Memory => "Memory",
            Column::WorkingSet => "Working set",
            Column::PeakWorkingSet => "Peak working set",
            Column::Commit => "Commit",
            Column::PageFaults => "Page faults",
            Column::Disk => "Disk",
            Column::Gpu => "GPU",
            Column::GpuMemory => "GPU memory",
            Column::Threads => "Threads",
            Column::Handles => "Handles",
            Column::Priority => "Priority",
            Column::Session => "Session",
            Column::ParentPid => "Parent",
            Column::Architecture => "Arch",
            Column::Elevated => "Elevated",
            Column::Virtualization => "UAC virtualization",
            Column::Description => "Description",
            Column::CommandLine => "Command line",
        }
    }

    fn width(self) -> f32 {
        match self {
            Column::Name => 160.0,
            Column::Pid | Column::Cpu | Column::Gpu | Column::Session => 58.0,
            Column::Status => 76.0,
            Column::User => 110.0,
            Column::CpuTime => 78.0,
            Column::Memory | Column::Disk | Column::WorkingSet | Column::Commit => 80.0,
            Column::PeakWorkingSet => 118.0,
            Column::PageFaults => 86.0,
            Column::GpuMemory => 92.0,
            Column::Threads => 58.0,
            Column::Handles | Column::ParentPid => 62.0,
            Column::Priority => 96.0,
            Column::Architecture => 52.0,
            Column::Elevated => 66.0,
            Column::Virtualization => 132.0,
            Column::Description => 130.0,
            Column::CommandLine => 200.0,
        }
    }

    fn numeric(self) -> bool {
        matches!(
            self,
            Column::Pid
                | Column::Cpu
                | Column::CpuTime
                | Column::Memory
                | Column::WorkingSet
                | Column::PeakWorkingSet
                | Column::Commit
                | Column::PageFaults
                | Column::Disk
                | Column::Gpu
                | Column::GpuMemory
                | Column::Threads
                | Column::Handles
                | Column::Session
                | Column::ParentPid
        )
    }

    fn flexible(self) -> bool {
        matches!(self, Column::Name | Column::Description | Column::CommandLine)
    }
}

/// Task Manager's sections: programs with a window, everything else, and Windows itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Category {
    Apps,
    Background,
    Windows,
}

impl Category {
    fn title(self) -> &'static str {
        match self {
            Category::Apps => "Apps",
            Category::Background => "Background processes",
            Category::Windows => "Windows processes",
        }
    }
}

/// What the toolbar and context menu act on.
#[derive(Clone, Debug, PartialEq)]
enum Selected {
    Process(Target),
    /// Every process of an executable, by lowercase name.
    Group(String),
}

/// An action waiting for confirmation in a dialog.
struct Pending {
    kind: Confirm,
    /// In the order to act on them (a tree's children before their parent).
    targets: Vec<Target>,
    label: String,
    critical: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    End,
    EndTree,
    Suspend,
    Realtime,
}

struct AffinityEdit {
    target: Target,
    label: String,
    mask: u64,
    available: u64,
}

struct RunDialog {
    command: String,
    admin: bool,
    error: Option<String>,
    focus: bool,
}

pub struct State {
    query: String,
    sort: Sort<Column>,
    grouped: bool,
    categories: bool,
    /// Lowercase names of expanded groups.
    expanded: HashSet<String>,
    collapsed: HashSet<Category>,
    selected: Option<Selected>,
    pending: Option<Pending>,
    affinity: Option<AffinityEdit>,
    run: Option<RunDialog>,
    dump: Option<(String, Job<Result<PathBuf, tasks::Error>>)>,
    notice: Option<Notice>,
    /// Services hosted by the process the context menu is open for.
    menu_services: Option<(u32, Vec<String>)>,
    /// Select this process and scroll to it once it is listed ("Go to details").
    focus: Option<u32>,
    scroll_to: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            query: String::new(),
            sort: Sort { column: Column::Cpu, descending: true },
            grouped: false,
            categories: false,
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            selected: None,
            pending: None,
            affinity: None,
            run: None,
            dump: None,
            notice: None,
            menu_services: None,
            focus: None,
            scroll_to: None,
        }
    }
}

impl State {
    /// Shows the process with this id the next time the page is drawn.
    pub fn focus(&mut self, pid: u32) {
        self.focus = Some(pid);
        self.query.clear();
    }
}

/// One line of the table.
enum Line<'a> {
    Category { category: Category, count: usize },
    Group { key: String, rows: Vec<&'a ProcessRow>, depth: u8 },
    Process { row: &'a ProcessRow, depth: u8 },
}

impl Line<'_> {
    fn rows(&self) -> Vec<&ProcessRow> {
        match self {
            Line::Process { row, .. } => vec![row],
            Line::Group { rows, .. } => rows.clone(),
            Line::Category { .. } => Vec::new(),
        }
    }

    fn selection(&self) -> Option<Selected> {
        match self {
            Line::Process { row, .. } => Some(Selected::Process(target(row))),
            Line::Group { key, .. } => Some(Selected::Group(key.clone())),
            Line::Category { .. } => None,
        }
    }
}

fn target(row: &ProcessRow) -> Target {
    Target { pid: row.process.pid, create_time: row.process.create_time }
}

fn shown_columns(settings: &Settings) -> Vec<Column> {
    if settings.process_columns.is_empty() {
        return Column::DEFAULT.to_vec();
    }
    let mut columns: Vec<Column> =
        Column::ALL.into_iter().filter(|c| settings.process_columns.iter().any(|k| k == c.key())).collect();
    if !columns.contains(&Column::Name) {
        columns.insert(0, Column::Name);
    }
    columns
}

pub fn page(ui: &mut Ui, model: &Model, settings: &mut Settings, state: &mut State, nav: &mut Option<Page>) {
    if let Some((name, job)) = &state.dump
        && let Some(result) = job.take()
    {
        state.notice = Some(match result {
            Ok(path) => {
                let path = path.display().to_string();
                Notice { text: format!("Dump file of {name} written to {path}"), denied: false, reveal: Some(path) }
            }
            Err(tasks::Error::AccessDenied) => Notice::denied(format!("Access denied: could not dump {name}.")),
            Err(tasks::Error::Gone) => Notice::info(format!("{name} has ended.")),
            Err(tasks::Error::Other(e)) => Notice::info(format!("Could not dump {name}: {e}")),
        });
        state.dump = None;
    }

    let Some(rows) = &model.processes else {
        widgets::waiting(ui, "Listing processes…", "Reading names, users and descriptions of every running process");
        return;
    };

    let categories = categorize(rows);
    let focused = state.focus.is_some();
    if let Some(pid) = state.focus.take() {
        match rows.iter().find(|r| r.process.pid == pid) {
            Some(row) => {
                state.selected = Some(Selected::Process(target(row)));
                let key = row.process.name.to_lowercase();
                state.collapsed.remove(&categories[&key]);
                state.expanded.insert(key);
            }
            None => state.notice = Some(Notice::info(format!("No process with ID {pid} is running."))),
        }
    }

    let query = state.query.trim().to_lowercase();
    let matches = |r: &ProcessRow| {
        query.is_empty()
            || r.process.name.to_lowercase().contains(&query)
            || r.process.pid.to_string().starts_with(&query)
            || r.facts.user.as_ref().is_some_and(|u| u.to_lowercase().contains(&query))
            || r.facts.description.as_ref().is_some_and(|d| d.to_lowercase().contains(&query))
            || r.window.as_ref().is_some_and(|w| w.title.to_lowercase().contains(&query))
    };
    let mut visible: Vec<&ProcessRow> = rows.iter().filter(|r| matches(r)).collect();
    let sort = state.sort;
    let order = |a: &ProcessRow, b: &ProcessRow| {
        sort.apply(compare(sort.column, a, b))
            .then_with(|| a.process.name.to_lowercase().cmp(&b.process.name.to_lowercase()))
            .then(a.process.pid.cmp(&b.process.pid))
    };
    visible.sort_by(|a, b| order(a, b));
    let lines = lines(&visible, state, &categories, &order);

    // Forget a selection whose processes ended.
    let selected_rows: Vec<&ProcessRow> = match &state.selected {
        Some(Selected::Process(t)) => rows.iter().filter(|r| target(r) == *t).collect(),
        Some(Selected::Group(key)) => rows.iter().filter(|r| r.process.name.to_lowercase() == *key).collect(),
        None => Vec::new(),
    };
    if selected_rows.is_empty() {
        state.selected = None;
    }
    if focused && let Some(Selected::Process(t)) = &state.selected {
        state.scroll_to = lines.iter().position(|l| matches!(l, Line::Process { row, .. } if target(row) == *t));
    }

    toolbar(ui, settings, state, rows.len(), visible.len(), &selected_rows);
    table::notice(ui, &mut state.notice, "processes");

    let (delete, escape) = ui.input(|i| (i.key_pressed(Key::Delete), i.key_pressed(Key::Escape)));
    let dialog_open = state.pending.is_some() || state.affinity.is_some() || state.run.is_some();
    if delete && !selected_rows.is_empty() && !dialog_open {
        request_end(state, &selected_rows, false, rows);
    }
    if escape && !dialog_open {
        state.selected = None;
    }

    let columns = shown_columns(settings);
    table::card(ui, |ui| {
        let columns = table::layout(&columns, &Column::HIDE_ORDER, ui.available_width() - 12.0);
        table::header(ui, &mut state.sort, &columns);
        let mut area = ScrollArea::vertical().id_salt("processes").auto_shrink(false);
        if let Some(index) = state.scroll_to.take() {
            area = area.vertical_scroll_offset((index as f32 - 4.0).max(0.0) * table::ROW);
        }
        area.show_rows(ui, table::ROW, lines.len(), |ui, range| {
            for line in &lines[range] {
                row(ui, settings, state, line, &columns, rows, nav);
            }
        });
    });

    confirm(ui, state);
    affinity_dialog(ui, state);
    run_dialog(ui, state);
}

/// Category per lowercase executable name. An executable with any window is an app,
/// with all its processes (browser helpers belong to the browser); one that lives in
/// the Windows folder (or whose path is hidden, like System) is part of Windows.
fn categorize(rows: &[ProcessRow]) -> HashMap<String, Category> {
    let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase() + "\\";
    let mut categories: HashMap<String, Category> = HashMap::new();
    for r in rows {
        let own = if r.window.is_some() {
            Category::Apps
        } else if r.facts.path.as_ref().is_none_or(|p| p.to_lowercase().starts_with(&windows_dir)) {
            Category::Windows
        } else {
            Category::Background
        };
        let entry = categories.entry(r.process.name.to_lowercase()).or_insert(own);
        // Apps wins over Background, which wins over Windows.
        *entry = (*entry).min(own);
    }
    categories
}

/// Sections when sorted into categories, groups per executable when grouped; a group
/// of one stays a plain row.
fn lines<'a>(
    visible: &[&'a ProcessRow],
    state: &State,
    categories: &HashMap<String, Category>,
    order: &dyn Fn(&ProcessRow, &ProcessRow) -> Ordering,
) -> Vec<Line<'a>> {
    let sections: Vec<(Option<Category>, Vec<&ProcessRow>)> = if state.categories {
        [Category::Apps, Category::Background, Category::Windows]
            .into_iter()
            .map(|c| {
                (Some(c), visible.iter().copied().filter(|r| categories[&r.process.name.to_lowercase()] == c).collect())
            })
            .filter(|(_, rows): &(_, Vec<_>)| !rows.is_empty())
            .collect()
    } else {
        vec![(None, visible.to_vec())]
    };

    let mut lines = Vec::new();
    for (category, rows) in sections {
        let depth = category.is_some() as u8;
        if let Some(category) = category {
            lines.push(Line::Category { category, count: rows.len() });
            if state.collapsed.contains(&category) {
                continue;
            }
        }
        if !state.grouped {
            lines.extend(rows.iter().map(|&row| Line::Process { row, depth }));
            continue;
        }
        let mut groups: Vec<(String, Vec<&ProcessRow>)> = Vec::new();
        for &row in &rows {
            let key = row.process.name.to_lowercase();
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, rows)) => rows.push(row),
                None => groups.push((key, vec![row])),
            }
        }
        // Groups by their total, using a stand-in row that holds the sums.
        let totals: Vec<ProcessRow> = groups.iter().map(|(_, rows)| sum(rows)).collect();
        let mut indices: Vec<usize> = (0..groups.len()).collect();
        indices.sort_by(|&a, &b| order(&totals[a], &totals[b]));
        for i in indices {
            let (key, rows) = &groups[i];
            if rows.len() == 1 {
                lines.push(Line::Process { row: rows[0], depth });
                continue;
            }
            lines.push(Line::Group { key: key.clone(), rows: rows.clone(), depth });
            if state.expanded.contains(key) {
                lines.extend(rows.iter().map(|&row| Line::Process { row, depth: depth + 1 }));
            }
        }
    }
    lines
}

/// A row with the summed values of `rows`, for sorting and showing a group.
fn sum(rows: &[&ProcessRow]) -> ProcessRow {
    let mut total = rows[0].clone();
    let total_of = |f: fn(&ProcessRow) -> u64| rows.iter().map(|r| f(r)).sum::<u64>();
    let p = &mut total.process;
    p.cpu = rows.iter().map(|r| r.process.cpu).sum();
    p.cpu_time = rows.iter().map(|r| r.process.cpu_time).sum();
    p.private_bytes = total_of(|r| r.process.private_bytes);
    p.working_set = total_of(|r| r.process.working_set);
    p.peak_working_set = total_of(|r| r.process.peak_working_set);
    p.commit = total_of(|r| r.process.commit);
    p.page_faults = rows.iter().map(|r| r.process.page_faults).sum();
    p.io_bytes_per_sec = rows.iter().map(|r| r.process.io_bytes_per_sec).sum();
    p.threads = rows.iter().map(|r| r.process.threads).sum();
    p.handles = rows.iter().map(|r| r.process.handles).sum();
    p.suspended = rows.iter().all(|r| r.process.suspended);
    total.gpu.load = rows.iter().map(|r| r.gpu.load).sum::<f64>().min(100.0);
    total.gpu.memory = total_of(|r| r.gpu.memory);
    total.efficiency = rows.iter().all(|r| r.efficiency);
    total.window = rows.iter().find_map(|r| r.window.clone());
    total
}

fn compare(column: Column, a: &ProcessRow, b: &ProcessRow) -> Ordering {
    let (pa, pb) = (&a.process, &b.process);
    let text = |x: &Option<String>, y: &Option<String>| {
        x.as_deref().unwrap_or("").to_lowercase().cmp(&y.as_deref().unwrap_or("").to_lowercase())
    };
    match column {
        Column::Name => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
        Column::Pid => pa.pid.cmp(&pb.pid),
        Column::Status => status(a).cmp(status(b)),
        Column::User => text(&a.facts.user, &b.facts.user),
        Column::Cpu => pa.cpu.total_cmp(&pb.cpu),
        Column::CpuTime => pa.cpu_time.cmp(&pb.cpu_time),
        Column::Memory => pa.private_bytes.cmp(&pb.private_bytes),
        Column::WorkingSet => pa.working_set.cmp(&pb.working_set),
        Column::PeakWorkingSet => pa.peak_working_set.cmp(&pb.peak_working_set),
        Column::Commit => pa.commit.cmp(&pb.commit),
        Column::PageFaults => pa.page_faults.cmp(&pb.page_faults),
        Column::Disk => pa.io_bytes_per_sec.total_cmp(&pb.io_bytes_per_sec),
        Column::Gpu => a.gpu.load.total_cmp(&b.gpu.load),
        Column::GpuMemory => a.gpu.memory.cmp(&b.gpu.memory),
        Column::Threads => pa.threads.cmp(&pb.threads),
        Column::Handles => pa.handles.cmp(&pb.handles),
        Column::Priority => pa.priority.cmp(&pb.priority),
        Column::Session => pa.session_id.cmp(&pb.session_id),
        Column::ParentPid => pa.parent_pid.cmp(&pb.parent_pid),
        Column::Architecture => a.facts.architecture.cmp(&b.facts.architecture),
        Column::Elevated => a.facts.elevated.cmp(&b.facts.elevated),
        Column::Virtualization => a.facts.virtualization.cmp(&b.facts.virtualization),
        Column::Description => text(&a.facts.description, &b.facts.description),
        Column::CommandLine => text(&a.facts.command_line, &b.facts.command_line),
    }
}

fn status(row: &ProcessRow) -> &'static str {
    if row.process.suspended {
        "Suspended"
    } else if row.efficiency {
        "Efficiency mode"
    } else {
        "Running"
    }
}

// ---------------------------------------------------------------------------
// Toolbar

fn toolbar(
    ui: &mut Ui,
    settings: &mut Settings,
    state: &mut State,
    total: usize,
    shown: usize,
    selected: &[&ProcessRow],
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        table::search(ui, &mut state.query, "Search name, PID, user, description or window", 250.0);
        if widgets::chip(ui, "Group by app", state.grouped, true, theme::ACCENT).clicked() {
            state.grouped = !state.grouped;
        }
        if widgets::chip(ui, "Sort by type", state.categories, true, theme::ACCENT).clicked() {
            state.categories = !state.categories;
        }
        columns_menu(ui, settings);
        table::count(ui, if shown == total { format!("{total} processes") } else { format!("{shown} of {total}") });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any = !selected.is_empty();
            if table::button(ui, "End task", any, true).clicked() {
                request_end(state, selected, false, &[]);
            }
            let suspended = any && selected.iter().all(|r| r.process.suspended);
            let (label, action) = if suspended { ("Resume", Action::Resume) } else { ("Suspend", Action::Suspend) };
            if table::button(ui, label, any, false).clicked() {
                request(state, action, selected);
            }
            let efficient = any && selected.iter().all(|r| r.efficiency);
            let label = if efficient { "Leave efficiency mode" } else { "Efficiency mode" };
            if table::button(ui, label, any, false).clicked() {
                request(state, Action::Efficiency(!efficient), selected);
            }
            if table::button(ui, "Run new task", true, false).clicked() {
                state.run = Some(RunDialog { command: String::new(), admin: false, error: None, focus: true });
            }
            table::admin_badge(ui);
        });
    });
    ui.add_space(-2.0);
}

/// "Columns" button with a checkbox per column; changes apply and persist at once.
fn columns_menu(ui: &mut Ui, settings: &mut Settings) {
    let response = table::button(ui, "Columns", true, false);
    egui::Popup::menu(&response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(220.0);
        let mut shown = shown_columns(settings);
        let mut changed = false;
        for column in Column::ALL.into_iter().filter(|c| *c != Column::Name) {
            let mut on = shown.contains(&column);
            let response = ui.checkbox(&mut on, column.title());
            let hint = column.hint();
            let response = if hint.is_empty() { response } else { response.on_hover_text(hint) };
            if response.changed() {
                if on {
                    shown.push(column);
                } else {
                    shown.retain(|c| *c != column);
                }
                changed = true;
            }
        }
        ui.separator();
        if ui.button("Default columns").clicked() {
            settings.process_columns.clear();
        } else if changed {
            settings.process_columns = Column::ALL
                .into_iter()
                .filter(|c| *c == Column::Name || shown.contains(c))
                .map(|c| c.key().to_owned())
                .collect();
        }
    });
}

// ---------------------------------------------------------------------------
// Table

fn row(
    ui: &mut Ui,
    settings: &Settings,
    state: &mut State,
    line: &Line,
    columns: &[(Column, f32)],
    all: &[ProcessRow],
    nav: &mut Option<Page>,
) {
    let selection = line.selection();
    let selected = selection.is_some() && state.selected == selection;
    let (rect, response, painter) = table::row(ui, selected);

    if let Line::Category { category, count } = line {
        if response.clicked() && !state.collapsed.remove(category) {
            state.collapsed.insert(*category);
        }
        let open = !state.collapsed.contains(category);
        let cell = rect.shrink2(egui::vec2(table::PAD, 0.0));
        table::name(&painter, cell, &format!("{}  ({count})", category.title()), 0, Some(open), true);
        return;
    }
    let selection = selection.expect("not a category");

    if response.clicked() || response.secondary_clicked() {
        state.selected = Some(selection.clone());
    }
    let depth = match line {
        Line::Group { depth, .. } | Line::Process { depth, .. } => *depth,
        Line::Category { .. } => 0,
    };
    if let Line::Group { key, .. } = line
        && (response.double_clicked() || response.clicked() && table::chevron_hit(ui, rect, depth))
        && !state.expanded.remove(key)
    {
        state.expanded.insert(key.clone());
    }
    if let (Line::Process { row, .. }, true) = (line, response.double_clicked())
        && let Some(window) = &row.window
    {
        switch_to(ui, settings, window.hwnd);
    }
    response.context_menu(|ui| {
        state.selected = Some(selection.clone());
        context_menu(ui, settings, state, &line.rows(), all, nav);
    });

    let (rows, group) = match line {
        Line::Process { row, .. } => (vec![*row], false),
        Line::Group { rows, .. } => (rows.clone(), true),
        Line::Category { .. } => unreachable!(),
    };
    let shown = if group { sum(&rows) } else { rows[0].clone() };
    let (p, f) = (&shown.process, &shown.facts);
    let elevated_column = columns.iter().any(|(c, _)| *c == Column::Elevated);

    for (column, cell) in table::cells(rect, columns) {
        let faint = |s: &str| (s.to_owned(), theme::TEXT_FAINT);
        let dim = |s: String| (s, theme::TEXT_DIM);
        let mib = |bytes: u64| (widgets::format_value(bytes as f64 / 1048576.0, Unit::Mebibytes), theme::TEXT);
        // A group shows other values (user, description …) only when all its processes share them.
        if group && !column.summed() && rows.iter().any(|r| compare(column, r, rows[0]) != Ordering::Equal) {
            continue;
        }
        let (text, color) = match column {
            Column::Name => {
                let name = if group { format!("{}  ×{}", p.name, rows.len()) } else { p.name.clone() };
                let chevron = match line {
                    Line::Group { key, .. } => Some(state.expanded.contains(key)),
                    _ => None,
                };
                table::name(&painter, cell, &name, depth, chevron, group);
                continue;
            }
            Column::Pid => dim(p.pid.to_string()),
            Column::Status => match status(&shown) {
                "Suspended" => ("Suspended".into(), theme::ACCENT),
                "Efficiency mode" => ("Efficiency".into(), theme::LIVE),
                s => dim(s.into()),
            },
            Column::User => match (&f.user, f.elevated) {
                (Some(user), Some(true)) if !elevated_column => dim(format!("{user} · admin")),
                (Some(user), _) => dim(user.clone()),
                (None, _) => faint("—"),
            },
            Column::Cpu if p.cpu < 0.05 => faint("0"),
            Column::Cpu => (format!("{:.1}%", p.cpu), theme::accent(Unit::Percent, p.cpu * 4.0)),
            Column::CpuTime => dim(cpu_time(p.cpu_time)),
            Column::Memory => mib(p.private_bytes),
            Column::WorkingSet => mib(p.working_set),
            Column::PeakWorkingSet => mib(p.peak_working_set),
            Column::Commit => mib(p.commit),
            Column::PageFaults => dim(thousands(p.page_faults as u64)),
            Column::Disk if p.io_bytes_per_sec < 512.0 => faint("0"),
            Column::Disk => (widgets::format_value(p.io_bytes_per_sec, Unit::BytesPerSec), theme::TEXT),
            Column::Gpu if shown.gpu.load < 0.05 => faint("0"),
            Column::Gpu => (format!("{:.1}%", shown.gpu.load), theme::accent(Unit::Percent, shown.gpu.load * 2.0)),
            Column::GpuMemory if shown.gpu.memory == 0 => faint("0"),
            Column::GpuMemory => mib(shown.gpu.memory),
            Column::Threads => dim(p.threads.to_string()),
            Column::Handles => dim(p.handles.to_string()),
            Column::Priority => match Priority::from_base(p.priority) {
                Some(priority) => dim(priority.label().into()),
                None => faint("—"),
            },
            Column::Session => dim(p.session_id.to_string()),
            Column::ParentPid if p.parent_pid == 0 => faint("—"),
            Column::ParentPid => dim(p.parent_pid.to_string()),
            Column::Architecture => match f.architecture {
                Some(a) => dim(a.to_owned()),
                None => faint("—"),
            },
            Column::Elevated => match f.elevated {
                Some(true) => ("Yes".into(), theme::TEXT),
                Some(false) => dim("No".into()),
                None => faint("—"),
            },
            Column::Virtualization => match f.virtualization {
                Some(crate::sensors::Virtualization::Enabled) => ("Enabled".into(), theme::TEXT),
                Some(crate::sensors::Virtualization::Disabled) => dim("Disabled".into()),
                Some(crate::sensors::Virtualization::NotAllowed) => dim("Not allowed".into()),
                None => faint("—"),
            },
            Column::Description => match &f.description {
                Some(d) => dim(d.clone()),
                None => faint(""),
            },
            Column::CommandLine => match &f.command_line {
                Some(c) => dim(c.clone()),
                None => faint(""),
            },
        };
        table::text(&painter, cell, column, &text, color);
    }
}

/// `1234567` → `1,234,567`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 100 ns units as `h:mm:ss`, like Task Manager.
fn cpu_time(units: i64) -> String {
    let s = units / 10_000_000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn context_menu(
    ui: &mut Ui,
    settings: &Settings,
    state: &mut State,
    rows: &[&ProcessRow],
    all: &[ProcessRow],
    nav: &mut Option<Page>,
) {
    let single = (rows.len() == 1).then(|| rows[0]);
    let mut close = false;

    if let Some(window) = rows.iter().find_map(|r| r.window.as_ref())
        && ui.button("Switch to").clicked()
    {
        switch_to(ui, settings, window.hwnd);
        close = true;
    }
    if ui.button("End task").clicked() {
        request_end(state, rows, false, all);
        close = true;
    }
    if single.is_some() && ui.button("End process tree").clicked() {
        request_end(state, rows, true, all);
        close = true;
    }
    let suspended = rows.iter().all(|r| r.process.suspended);
    let (label, action) = if suspended { ("Resume", Action::Resume) } else { ("Suspend", Action::Suspend) };
    if ui.button(label).clicked() {
        request(state, action, rows);
        close = true;
    }
    let efficient = rows.iter().all(|r| r.efficiency);
    // Selected (highlighted) items mark what is on; the UI fonts have no check mark.
    if ui.add(egui::Button::new("Efficiency mode").selected(efficient)).clicked() {
        request(state, Action::Efficiency(!efficient), rows);
        close = true;
    }
    ui.menu_button("Set priority", |ui| {
        let current = single.and_then(|r| Priority::from_base(r.process.priority));
        for priority in Priority::ALL {
            if ui.add(egui::Button::new(priority.label()).selected(current == Some(priority))).clicked() {
                request(state, Action::Priority(priority), rows);
                ui.close();
            }
        }
    });
    if let Some(row) = single
        && ui.button("Set affinity…").clicked()
    {
        match tasks::affinity(target(row)) {
            Some((mask, available)) => {
                state.affinity =
                    Some(AffinityEdit { target: target(row), label: row.process.name.clone(), mask, available })
            }
            None => state.notice = Some(Notice::denied(format!("Access denied: could not read {}.", row.process.name))),
        }
        close = true;
    }
    ui.separator();
    if let Some(row) = single {
        if ui.button("Create dump file").clicked() && state.dump.is_none() {
            let (t, name) = (target(row), row.process.name.clone());
            let job = Job::start(ui.ctx(), move || tasks::dump(t, &name));
            state.notice = Some(Notice::info(format!("Writing a dump file of {}…", row.process.name)));
            state.dump = Some((row.process.name.clone(), job));
            close = true;
        }
        let pid = row.process.pid;
        if state.menu_services.as_ref().is_none_or(|(p, _)| *p != pid) {
            state.menu_services = Some((pid, crate::services::for_pid(pid)));
        }
        if let Some((_, services)) = &state.menu_services
            && !services.is_empty()
        {
            let label = if services.len() == 1 {
                "Go to service".to_owned()
            } else {
                format!("Go to services ({})", services.len())
            };
            if ui.button(label).clicked() {
                *nav = Some(Page::ServicesOf(pid));
                close = true;
            }
        }
    }
    let path = rows[0].facts.path.clone();
    if let Some(path) = &path {
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
        let name = &rows[0].process.name;
        let query = match &rows[0].facts.description {
            Some(d) if !d.eq_ignore_ascii_case(name) => format!("{name} {d}"),
            _ => name.clone(),
        };
        shell::search_online(&query);
        close = true;
    }
    ui.separator();
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(rows[0].process.name.clone());
        close = true;
    }
    if let Some(path) = path
        && ui.button("Copy path").clicked()
    {
        ui.ctx().copy_text(path);
        close = true;
    }
    if let Some(command) = single.and_then(|r| r.facts.command_line.clone())
        && ui.button("Copy command line").clicked()
    {
        ui.ctx().copy_text(command);
        close = true;
    }
    if close {
        ui.close();
    }
}

fn switch_to(ui: &Ui, settings: &Settings, hwnd: isize) {
    if shell::switch_to(hwnd) && settings.minimize_on_use {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}

// ---------------------------------------------------------------------------
// Actions

/// Ending always asks for confirmation. With `tree`, everything the process started
/// (recursively) goes too, children first.
fn request_end(state: &mut State, rows: &[&ProcessRow], tree: bool, all: &[ProcessRow]) {
    let mut targets: Vec<Target> = rows.iter().map(|r| target(r)).collect();
    if tree {
        let mut descendants = Vec::new();
        let mut frontier: Vec<&ProcessRow> = rows.to_vec();
        while let Some(parent) = frontier.pop() {
            // A child started after its parent; an older process with a matching
            // parent id belonged to an earlier process with the reused id.
            for child in all.iter().filter(|c| {
                c.process.parent_pid == parent.process.pid
                    && c.process.create_time >= parent.process.create_time
                    && c.process.pid != parent.process.pid
            }) {
                if !descendants.iter().any(|d: &&ProcessRow| d.process.pid == child.process.pid) {
                    descendants.push(child);
                    frontier.push(child);
                }
            }
        }
        let mut tree_targets: Vec<Target> = descendants.iter().rev().map(|r| target(r)).collect();
        tree_targets.append(&mut targets);
        targets = tree_targets;
    }
    let mut label = label(rows);
    if tree && targets.len() > 1 {
        label = format!("{label} and {} processes it started", targets.len() - 1);
    }
    let critical = critical(rows, false);
    let kind = if tree { Confirm::EndTree } else { Confirm::End };
    state.pending = Some(Pending { kind, targets, label, critical });
}

fn label(rows: &[&ProcessRow]) -> String {
    let name = rows.first().map(|r| r.process.name.clone()).unwrap_or_default();
    if rows.len() > 1 { format!("{} processes of {name}", rows.len()) } else { name }
}

/// Windows' own flag where readable, else well-known names. Suspending the desktop
/// window manager freezes the screen, hwmon included.
fn critical(rows: &[&ProcessRow], suspend: bool) -> bool {
    const KNOWN: [&str; 8] =
        ["System", "Registry", "smss.exe", "csrss.exe", "wininit.exe", "services.exe", "lsass.exe", "winlogon.exe"];
    rows.iter().any(|r| match tasks::is_critical(target(r)) {
        Some(flag) => flag,
        None => KNOWN.iter().any(|n| r.process.name.eq_ignore_ascii_case(n)),
    }) || suspend && rows.iter().any(|r| r.process.name.eq_ignore_ascii_case("dwm.exe"))
}

/// Suspending asks only for processes Windows cannot do without, realtime priority
/// always (it can starve the whole system); everything else runs right away.
fn request(state: &mut State, action: Action, rows: &[&ProcessRow]) {
    let mut targets: Vec<Target> = rows.iter().map(|r| target(r)).collect();
    let label = label(rows);
    if action == Action::Suspend {
        // Nothing could resume hwmon once it is frozen.
        let own = std::process::id();
        if targets.iter().any(|t| t.pid == own) {
            state.notice = Some(Notice::info("hwmon cannot suspend itself, as nothing could resume it."));
            targets.retain(|t| t.pid != own);
            if targets.is_empty() {
                return;
            }
        }
        if critical(rows, true) {
            state.pending = Some(Pending { kind: Confirm::Suspend, targets, label, critical: true });
            return;
        }
    }
    if action == Action::Priority(Priority::Realtime) {
        state.pending = Some(Pending { kind: Confirm::Realtime, targets, label, critical: false });
        return;
    }
    execute(state, action, &targets, &label);
}

fn execute(state: &mut State, action: Action, targets: &[Target], label: &str) {
    let mut denied = 0;
    let mut other = None;
    for &t in targets {
        match tasks::run(action, t) {
            Ok(()) | Err(tasks::Error::Gone) => {}
            Err(tasks::Error::AccessDenied) => denied += 1,
            Err(tasks::Error::Other(e)) => other = Some(e),
        }
    }
    let verb = match action {
        Action::End => "end".to_owned(),
        Action::Suspend => "suspend".to_owned(),
        Action::Resume => "resume".to_owned(),
        Action::Priority(p) => format!("set the priority of {label} to {}", p.label().to_lowercase()),
        Action::Efficiency(true) => "put efficiency mode on for".to_owned(),
        Action::Efficiency(false) => "take efficiency mode off for".to_owned(),
        Action::Affinity(_) => "change the processors of".to_owned(),
    };
    let what = if matches!(action, Action::Priority(_)) { verb } else { format!("{verb} {label}") };
    state.notice = if denied > 0 {
        let hint = if tasks::is_elevated() {
            "It is protected by Windows."
        } else {
            "It runs as administrator, as SYSTEM or as another user."
        };
        Some(Notice::denied(format!("Access denied: could not {what}. {hint}")))
    } else {
        other.map(|e| Notice::info(format!("Could not {what}: {e}")))
    };
}

fn confirm(ui: &mut Ui, state: &mut State) {
    let Some(pending) = &state.pending else { return };
    let (title, note, warning, button) = match pending.kind {
        Confirm::Suspend => (
            format!("Suspend {}?", pending.label),
            "It stops until resumed.",
            "This process is essential to Windows. Suspending it can freeze the whole system, \
             hwmon included, so it may not be possible to resume it.",
            "Suspend anyway",
        ),
        Confirm::Realtime => (
            format!("Run {} at realtime priority?", pending.label),
            "Without administrator rights Windows gives it high priority instead.",
            "Realtime priority can take the processor away from everything else, including \
             mouse and keyboard input, until the process yields.",
            "Set realtime",
        ),
        Confirm::EndTree => (
            format!("End {}?", pending.label),
            "Unsaved data in them is lost.",
            "This is a critical Windows process. Ending it crashes Windows.",
            "End process tree",
        ),
        Confirm::End => (
            format!("End {}?", pending.label),
            "Unsaved data in it is lost.",
            "This is a critical Windows process. Ending it crashes Windows.",
            "End process",
        ),
    };
    let warning = (pending.critical || pending.kind == Confirm::Realtime).then_some(warning);
    let Some(go) = table::confirm(ui, "confirm_action", &title, note, warning, button) else { return };
    let pending = state.pending.take().expect("checked above");
    if go {
        let action = match pending.kind {
            Confirm::End | Confirm::EndTree => Action::End,
            Confirm::Suspend => Action::Suspend,
            Confirm::Realtime => Action::Priority(Priority::Realtime),
        };
        execute(state, action, &pending.targets, &pending.label);
    }
}

/// A checkbox per logical processor, like Task Manager's "Processor affinity".
fn affinity_dialog(ui: &mut Ui, state: &mut State) {
    let Some(edit) = &mut state.affinity else { return };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::NULL.with("affinity")).show(ui.ctx(), |ui| {
        ui.set_width(420.0);
        ui.label(egui::RichText::new(format!("Processors for {}", edit.label)).font(theme::semibold(16.0)));
        ui.add_space(-4.0);
        ui.label(
            egui::RichText::new("It only runs on the checked logical processors.")
                .font(theme::regular(12.5))
                .color(theme::TEXT_DIM),
        );
        let all = edit.mask & edit.available == edit.available;
        let mut everything = all;
        if ui.checkbox(&mut everything, "All processors").changed() {
            edit.mask = if everything { edit.available } else { 0 };
        }
        let cpus: Vec<u32> = (0..64).filter(|i| edit.available & (1 << i) != 0).collect();
        egui::Grid::new("affinity_grid").num_columns(4).spacing([16.0, 6.0]).show(ui, |ui| {
            for (n, cpu) in cpus.iter().enumerate() {
                let mut on = edit.mask & (1 << cpu) != 0;
                if ui.checkbox(&mut on, format!("CPU {cpu}")).changed() {
                    edit.mask ^= 1 << cpu;
                }
                if n % 4 == 3 {
                    ui.end_row();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if table::button(ui, "Apply", edit.mask != 0, false).clicked() {
                    decision = Some(true);
                }
                if table::button(ui, "Cancel", true, false).clicked() {
                    decision = Some(false);
                }
            });
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    if let Some(go) = decision {
        let edit = state.affinity.take().expect("checked above");
        if go {
            execute(state, Action::Affinity(edit.mask), &[edit.target], &edit.label);
        }
    }
}

/// Task Manager's "Create new task": a program, document, folder or URL.
fn run_dialog(ui: &mut Ui, state: &mut State) {
    let Some(run) = &mut state.run else { return };
    let mut decision = None;
    let elevated = tasks::is_elevated();
    let modal = egui::Modal::new(egui::Id::NULL.with("run_task")).show(ui.ctx(), |ui| {
        ui.set_width(420.0);
        ui.label(egui::RichText::new("Run new task").font(theme::semibold(16.0)));
        ui.add_space(-4.0);
        ui.label(
            egui::RichText::new("The name of a program, folder, document or website, as in the Run dialog.")
                .font(theme::regular(12.5))
                .color(theme::TEXT_DIM),
        );
        let field = table::search(ui, &mut run.command, "e.g. notepad, cmd, %TEMP% or a web address", 400.0);
        if run.focus {
            field.request_focus();
            run.focus = false;
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            decision = Some(true);
        }
        if elevated {
            ui.label(
                egui::RichText::new("It runs as administrator, like hwmon.")
                    .font(theme::regular(12.5))
                    .color(theme::LIVE),
            );
        } else {
            ui.checkbox(&mut run.admin, "Create this task with administrative privileges");
        }
        if let Some(error) = &run.error {
            ui.label(egui::RichText::new(error).font(theme::regular(12.5)).color(theme::ERROR));
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if table::button(ui, "Run", !run.command.trim().is_empty(), false).clicked() {
                    decision = Some(true);
                }
                if table::button(ui, "Cancel", true, false).clicked() {
                    decision = Some(false);
                }
            });
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => match shell::run(&run.command, run.admin && !elevated) {
            Ok(()) => state.run = None,
            Err(e) => run.error = Some(e),
        },
        Some(false) => state.run = None,
        None => {}
    }
}
