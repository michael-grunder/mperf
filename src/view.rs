use crate::{
    cli::{Sort, ViewOptions},
    profile::{self, Entry, Key, Profile},
};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame as TerminalFrame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState, Wrap},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    io::{self, Write},
    time::Duration,
};

const HELP: &str = "mperf report / diff — keyboard guide\n\n\
Navigation (perf-compatible)\n\
  Up/Down, PgUp/PgDn, Space    Move / page\n\
  Home/End                    First / last row\n\
  q / Esc                     Close popup, clear zoom/filter, or quit\n\
  Ctrl-C                      Quit\n\
  h / ? / F1                  This help (scroll with arrows/pages)\n\
  + / e                       Expand/collapse selected symbol's call chains\n\
  E / C                       Expand / collapse all call chains\n\
  d                           Zoom into selected DSO (library/image)\n\
  /                           Filter symbol names; Enter applies, Esc cancels\n\
  H                           Toggle column headers\n\
  i                           Recording metadata and weight totals\n\n\
mperf additions and differences\n\
  j                           Move down (k is reserved for perf kernel zoom)\n\
  Enter                       Symbol details with complete sampled stacks\n\
  o                           Cycle sort: overhead/self/symbol/DSO/comm/delta\n\
  w                           Toggle self / inclusive overhead\n\
  Call chains expand as complete paths, leaf <- caller <- root.\n\
  Diff shows NEW recording call chains; removed symbols have no new stacks.\n\
  Narrow terminals hide process/image columns; Enter shows full details.\n\
  Assembly annotation, thread/kernel zoom, events and perf scripts are not implemented.\n\n\
Reading the numbers\n\
  Overhead = share of total exported sample weight, not wall time.\n\
  Self = sampled leaf; inclusive counts each symbol once per stack.\n\
  Inclusive rows overlap and need not sum to 100%.\n\
  Filters/zoom keep the original recording's denominator.\n\
  Diff delta = new minus baseline, in percentage points (pp).\n\
  A positive delta is a larger share; it does not prove a slowdown.\n\
  Unknown stacks remain in the denominator; see i for their count.\n\n\
xctrace capabilities\n\
  record --template / --instrument combines Instruments data in one trace.\n\
  record --device selects a Mac, simulator, or connected device.\n\
  record --time-limit / --window bounds recording duration / retained history.\n\
  report --run / --schema selects CPU data in external traces.\n\
  Non-CPU instruments remain in the trace; this viewer analyzes CPU samples.\n\
  See 'mperf record help' and 'mperf report help' for command examples.";

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' {
                '�'
            } else {
                c
            }
        })
        .collect()
}

fn initial_sort(options: &ViewOptions, diff: bool) -> Sort {
    options
        .sort
        .unwrap_or(if diff { Sort::Delta } else { Sort::Overhead })
}

pub fn print(profile: &Profile, baseline: Option<&Profile>, options: &ViewOptions) -> Result<()> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(output, "{}", clean(&profile.metadata()))?;
    if let Some(before) = baseline {
        writeln!(output, "Baseline: {}", clean(&before.metadata()))?;
    }
    writeln!(
        output,
        "Overhead: {} | filters keep full-recording denominators",
        if options.children {
            "inclusive"
        } else {
            "self"
        }
    )?;
    if baseline.is_some() {
        writeln!(
            output,
            "Baseline%     New%  Delta(pp)    Self%  Command  Shared Object  Symbol"
        )?;
    } else {
        writeln!(output, "Overhead%    Self%  Command  Shared Object  Symbol")?;
    }
    let mut entries = profile::entries(profile, baseline, options.children);
    profile::filter_entries(&mut entries, options);
    profile::sort_entries(&mut entries, initial_sort(options, baseline.is_some()));
    if entries.is_empty() {
        writeln!(output, "No symbols match the filters.")?;
    }
    for entry in entries.iter().take(options.limit as usize) {
        if baseline.is_some() {
            write!(
                output,
                "{:9.2} {:8.2} {:+10.2} ",
                entry.baseline, entry.overhead, entry.delta
            )?;
        } else {
            write!(output, "{:9.2} ", entry.overhead)?;
        }
        writeln!(
            output,
            "{:8.2}  {}  {}  {}",
            entry.self_percent,
            clean(&entry.key.process),
            clean(&entry.key.frame.image),
            clean(&entry.key.frame.symbol)
        )?;
    }
    output.flush()?;
    Ok(())
}

struct Popup {
    title: String,
    text: String,
    scroll: u16,
}

struct App<'a> {
    profile: &'a Profile,
    baseline: Option<&'a Profile>,
    options: &'a ViewOptions,
    entries: Vec<Entry>,
    expanded: BTreeSet<Key>,
    chains: BTreeMap<Key, Vec<(String, f64)>>,
    state: TableState,
    row_entries: Vec<usize>,
    children: bool,
    sort: Sort,
    filter: String,
    edit: Option<String>,
    dso: Option<String>,
    headers: bool,
    popup: Option<Popup>,
    message: String,
    color: bool,
    page_size: usize,
}

impl<'a> App<'a> {
    fn new(profile: &'a Profile, baseline: Option<&'a Profile>, options: &'a ViewOptions) -> Self {
        let mut app = Self {
            profile,
            baseline,
            options,
            entries: Vec::new(),
            expanded: BTreeSet::new(),
            chains: BTreeMap::new(),
            state: TableState::default().with_selected(0),
            row_entries: Vec::new(),
            children: options.children,
            sort: initial_sort(options, baseline.is_some()),
            filter: String::new(),
            edit: None,
            dso: None,
            headers: true,
            popup: None,
            message: String::new(),
            color: std::env::var_os("NO_COLOR").is_none(),
            page_size: 10,
        };
        app.rebuild();
        app
    }
    fn rebuild(&mut self) {
        self.entries = profile::entries(self.profile, self.baseline, self.children);
        profile::filter_entries(&mut self.entries, self.options);
        self.entries.retain(|e| {
            profile::matches(&e.key.frame.symbol, Some(&self.filter))
                && self.dso.as_ref().is_none_or(|d| d == &e.key.frame.image)
        });
        profile::sort_entries(&mut self.entries, self.sort);
        self.state.select(Some(0));
        self.chains.clear();
    }
    fn selected(&self) -> Option<&Entry> {
        self.state
            .selected()
            .and_then(|i| self.row_entries.get(i))
            .and_then(|i| self.entries.get(*i))
    }
    fn popup(&mut self, title: &str, text: &str) {
        self.popup = Some(Popup {
            title: title.into(),
            text: clean(text),
            scroll: 0,
        });
    }
    fn move_by(&mut self, amount: isize) {
        let last = self.row_entries.len().saturating_sub(1);
        self.state.select(Some(
            self.state
                .selected()
                .unwrap_or(0)
                .saturating_add_signed(amount)
                .min(last),
        ));
    }
    fn key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.edit.is_some() {
            self.edit_key(key.code);
            return false;
        }
        if self.popup.is_some() {
            self.popup_key(key.code);
            return false;
        }
        self.message.clear();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.dso.take().is_some() {
                    self.rebuild();
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild();
                } else {
                    return true;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.move_by(isize::try_from(self.page_size).unwrap_or(isize::MAX));
            }
            KeyCode::PageUp => self.move_by(-isize::try_from(self.page_size).unwrap_or(isize::MAX)),
            KeyCode::Home => self.state.select(Some(0)),
            KeyCode::End => self
                .state
                .select(Some(self.row_entries.len().saturating_sub(1))),
            KeyCode::Char('h' | '?') | KeyCode::F(1) => {
                self.popup(" Help — arrows/PgUp/PgDn scroll, q closes ", HELP);
            }
            KeyCode::Char('i') => {
                let mut text = self.profile.metadata();
                if let Some(before) = self.baseline {
                    let _ = write!(text, "\n\nBaseline:\n{}", before.metadata());
                }
                self.popup(" Recording metadata ", &text);
            }
            KeyCode::Char('H') => self.headers = !self.headers,
            KeyCode::Char('/') => self.edit = Some(self.filter.clone()),
            KeyCode::Char('d') => {
                if let Some(entry) = self.selected() {
                    self.dso = Some(entry.key.frame.image.clone());
                    self.rebuild();
                }
            }
            KeyCode::Char('+' | 'e') => {
                if let Some(key) = self.selected().map(|e| e.key.clone())
                    && !self.expanded.remove(&key)
                {
                    self.expanded.insert(key);
                }
            }
            KeyCode::Char('E') => self
                .expanded
                .extend(self.entries.iter().map(|e| e.key.clone())),
            KeyCode::Char('C') => self.expanded.clear(),
            KeyCode::Char('w') => {
                self.children = !self.children;
                self.rebuild();
            }
            KeyCode::Char('o') => {
                self.sort = next_sort(self.sort, self.baseline.is_some());
                self.rebuild();
            }
            KeyCode::Enter => self.details(),
            KeyCode::Char('a' | 't' | 'k' | 's' | 'r' | 'F' | 'L' | 'm') => {
                self.message =
                    "This perf action is not implemented; press h for supported keys.".into();
            }
            _ => (),
        }
        false
    }
    fn details(&mut self) {
        if let Some(entry) = self.selected() {
            let mut text = format!(
                "{}\nProcess: {}\nImage: {}\nNew overhead: {:.2}% | self: {:.2}%\n",
                entry.key.frame.symbol,
                entry.key.process,
                entry.key.frame.image,
                entry.overhead,
                entry.self_percent
            );
            if self.baseline.is_some() {
                let _ = writeln!(
                    text,
                    "Baseline: {:.2}% | delta: {:+.2} pp",
                    entry.baseline, entry.delta
                );
            }
            text.push_str("\nCall chains (new recording, leaf to root):\n");
            for (chain, percent) in self.profile.callchains(&entry.key, self.children) {
                let _ = writeln!(
                    text,
                    "\n{percent:.2}% {}",
                    chain.replace(" <- ", "\n    <- ")
                );
            }
            self.popup(" Symbol details — arrows/PgUp/PgDn scroll ", &text);
        }
    }
    fn edit_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                self.filter = self.edit.take().unwrap_or_default();
                self.rebuild();
            }
            KeyCode::Esc => {
                self.edit = None;
            }
            KeyCode::Backspace => {
                if let Some(edit) = &mut self.edit {
                    edit.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(edit) = &mut self.edit {
                    edit.push(c);
                }
            }
            _ => (),
        }
    }
    fn popup_key(&mut self, code: KeyCode) {
        if matches!(
            code,
            KeyCode::Esc | KeyCode::Char('q' | 'h' | '?') | KeyCode::F(1)
        ) {
            self.popup = None;
            return;
        }
        if let Some(popup) = &mut self.popup {
            match code {
                KeyCode::Down | KeyCode::Char('j') => popup.scroll = popup.scroll.saturating_add(1),
                KeyCode::Up => popup.scroll = popup.scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    popup.scroll = popup.scroll.saturating_add(10);
                }
                KeyCode::PageUp => popup.scroll = popup.scroll.saturating_sub(10),
                KeyCode::Home => popup.scroll = 0,
                _ => (),
            }
        }
    }
    fn draw(&mut self, frame: &mut TerminalFrame<'_>) {
        let [heading, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        self.page_size = body.height.saturating_sub(3).max(1).into();
        let diff = self.baseline.is_some();
        let title = format!(
            "mperf {} | {} samples | {} | sort {:?}\n{} | filter: {} | DSO: {}",
            if diff { "diff (delta in pp)" } else { "report" },
            self.profile.samples,
            if self.children { "inclusive" } else { "self" },
            self.sort,
            clean(&self.profile.source),
            self.filter,
            self.dso.as_deref().unwrap_or("all")
        );
        frame.render_widget(
            Paragraph::new(title)
                .style(self.accent())
                .block(Block::default().borders(Borders::BOTTOM)),
            heading,
        );
        let columns: &[usize] = match (diff, body.width) {
            (true, 0..100) => &[0, 1, 2, 6],
            (true, _) => &[0, 1, 2, 3, 4, 5, 6],
            (false, 0..75) => &[0, 1, 4],
            (false, _) => &[0, 1, 2, 3, 4],
        };
        let rows = self.rows(columns);
        let all_widths = if diff {
            vec![
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(16),
                Constraint::Length(18),
                Constraint::Min(20),
            ]
        } else {
            vec![
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(16),
                Constraint::Length(18),
                Constraint::Min(20),
            ]
        };
        let widths: Vec<_> = columns.iter().map(|i| all_widths[*i]).collect();
        let mut table = Table::new(rows, widths)
            .column_spacing(1)
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .block(Block::bordered().title(" CPU symbols — + expands call chains "));
        if self.headers {
            let labels = if diff {
                vec![
                    "Baseline",
                    "New",
                    "Delta pp",
                    "Self",
                    "Command",
                    "Shared Object",
                    "Symbol",
                ]
            } else {
                vec!["Overhead", "Self", "Command", "Shared Object", "Symbol"]
            };
            table = table.header(
                Row::new(columns.iter().map(|i| labels[*i]))
                    .style(self.accent())
                    .bottom_margin(1),
            );
        }
        frame.render_stateful_widget(table, body, &mut self.state);
        let footer_text = if let Some(edit) = &self.edit {
            format!("/{edit}\nEnter applies; Esc cancels")
        } else if !self.message.is_empty() {
            self.message.clone()
        } else if self.entries.is_empty() {
            "No symbols match filters. Esc clears interactive filters. h help | q quit".into()
        } else {
            format!(
                "h/? help | arrows move | +/e stacks | / filter | d DSO | Enter details | o sort | w self/inclusive | q back/quit\n{} symbols | {} samples without stacks (included in denominator)",
                self.entries.len(),
                self.profile.missing_stacks
            )
        };
        frame.render_widget(Paragraph::new(footer_text), footer);
        self.draw_popup(frame);
    }
    fn draw_popup(&mut self, frame: &mut TerminalFrame<'_>) {
        if let Some(popup) = &mut self.popup {
            let area = popup_area(frame.area());
            frame.render_widget(Clear, area);
            let paragraph = Paragraph::new(popup.text.clone()).wrap(Wrap { trim: false });
            let lines = paragraph.line_count(area.width.saturating_sub(2));
            popup.scroll = popup.scroll.min(
                u16::try_from(lines.saturating_sub(usize::from(area.height.saturating_sub(2))))
                    .unwrap_or(u16::MAX),
            );
            frame.render_widget(
                paragraph
                    .scroll((popup.scroll, 0))
                    .block(Block::bordered().title(popup.title.clone())),
                area,
            );
        }
    }
    fn accent(&self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.color {
            style.fg(Color::Cyan)
        } else {
            style
        }
    }
    fn rows(&mut self, columns: &[usize]) -> Vec<Row<'static>> {
        self.row_entries.clear();
        let mut rows = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let mut cells = Vec::new();
            if self.baseline.is_some() {
                cells.extend([
                    format!("{:7.2}%", entry.baseline),
                    format!("{:7.2}%", entry.overhead),
                    format!("{:+8.2}", entry.delta),
                ]);
            } else {
                cells.push(format!("{:7.2}%", entry.overhead));
            }
            cells.extend([
                format!("{:7.2}%", entry.self_percent),
                clean(&entry.key.process),
                clean(&entry.key.frame.image),
                format!(
                    "{} {}",
                    if self.expanded.contains(&entry.key) {
                        "-"
                    } else {
                        "+"
                    },
                    clean(&entry.key.frame.symbol)
                ),
            ]);
            let mut row = Row::new(columns.iter().map(|i| cells[*i].clone()));
            if self.color && self.baseline.is_some() {
                row = row.style(Style::default().fg(if entry.delta > 0.0 {
                    Color::Red
                } else if entry.delta < 0.0 {
                    Color::Green
                } else {
                    Color::Reset
                }));
            }
            rows.push(row);
            self.row_entries.push(index);
            if self.expanded.contains(&entry.key) {
                let chains = self
                    .chains
                    .entry(entry.key.clone())
                    .or_insert_with(|| self.profile.callchains(&entry.key, self.children));
                for (chain, percent) in chains {
                    let count = columns.len();
                    let mut cells = vec![String::new(); count];
                    cells[usize::from(self.baseline.is_some())] = format!("{percent:7.2}%");
                    cells[count - 1] = format!("  {}", clean(chain));
                    rows.push(Row::new(cells));
                    self.row_entries.push(index);
                }
            }
        }
        if let Some(selected) = self.state.selected() {
            self.state
                .select(Some(selected.min(self.row_entries.len().saturating_sub(1))));
        }
        rows
    }
}

const fn next_sort(sort: Sort, diff: bool) -> Sort {
    match sort {
        Sort::Overhead => Sort::SelfWeight,
        Sort::SelfWeight => Sort::Symbol,
        Sort::Symbol => Sort::Dso,
        Sort::Dso => Sort::Comm,
        Sort::Comm if diff => Sort::Delta,
        Sort::Comm | Sort::Delta => Sort::Overhead,
    }
}

const fn popup_area(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn interactive(
    profile: &Profile,
    baseline: Option<&Profile>,
    options: &ViewOptions,
) -> Result<()> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])?;
    let mut terminal = ratatui::try_init()?;
    let _restore = Restore;
    run(
        &mut terminal,
        &mut App::new(profile, baseline, options),
        &mut signals,
    )
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App<'_>,
    signals: &mut signal_hook::iterator::Signals,
) -> Result<()> {
    loop {
        if signals.pending().next().is_some() {
            return Ok(());
        }
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release && app.key(key) => {
                    return Ok(());
                }
                _ => (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn tui_renders_navigates_filters_and_opens_help() {
        let profile = crate::trace::parse(
            include_str!("../tests/fixtures/before.xml"),
            "demo.xml".into(),
            None,
        )
        .unwrap();
        let Command::Report(args) = Cli::parse_from(["mperf", "report"]).command else {
            panic!()
        };
        let mut app = App::new(&profile, None, &args.view);
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.row_entries.len(), 2);
        app.key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.row_entries.len(), 3);
        app.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert!(app.popup.as_ref().unwrap().text.contains("keyboard guide"));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.filter = "missing-symbol".into();
        app.rebuild();
        terminal.draw(|f| app.draw(f)).unwrap();
        assert!(app.entries.is_empty());
        assert!(!app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.entries.len(), 2);
        terminal.resize(Rect::new(0, 0, 15, 4)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
    }
}
