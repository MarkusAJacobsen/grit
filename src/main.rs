use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};
use similar::{ChangeTag, TextDiff};
use std::collections::VecDeque;

struct CommitInfo {
    hash: String,
    author: String,
    date: String,
    message: String,
    oid: gix::ObjectId,
}

enum View {
    CommitList,
    DiffView,
}

#[derive(PartialEq)]
enum DiffFocus {
    Content,
    FilePane,
}

enum DiffLine {
    Context(String),
    Changed {
        left: Option<String>,
        right: Option<String>,
    },
}

struct FileDiff {
    path: String,
    lines_added: usize,
    lines_removed: usize,
    rows: Vec<DiffLine>,
    collapsed: bool,
}

struct CommitDiff {
    files: Vec<FileDiff>,
}

struct App {
    commits: Vec<CommitInfo>,
    list_state: ListState,
    repo: gix::Repository,
    view: View,
    diff: Option<CommitDiff>,
    diff_scroll: usize,
    diff_total_lines: usize,
    diff_viewport_height: usize,
    diff_focus: DiffFocus,
    file_pane_open: bool,
    file_pane_state: ListState,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal) -> color_eyre::Result<()> {
    let repo = gix::open(".")?;
    let head_id = repo.head_id()?;
    let commits: Vec<CommitInfo> = repo
        .rev_walk([head_id])
        .all()?
        .filter_map(|info| info.ok())
        .filter_map(|info| {
            let commit = repo.find_commit(info.id).ok()?;
            let author = commit.author().ok()?;
            let hash = info.id.to_hex_with_len(7).to_string();
            let msg = commit.message_raw_sloppy();
            let first_line = String::from_utf8_lossy(
                msg.split(|&b| b == b'\n').next().unwrap_or_default(),
            )
            .into_owned();
            let dt = chrono::DateTime::from_timestamp(author.time.seconds, 0)?;
            Some(CommitInfo {
                hash,
                author: String::from_utf8_lossy(author.name).into_owned(),
                date: dt.format("%Y-%m-%d").to_string(),
                message: first_line,
                oid: info.id,
            })
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let mut app = App {
        commits,
        list_state,
        repo,
        view: View::CommitList,
        diff: None,
        diff_scroll: 0,
        diff_total_lines: 0,
        diff_viewport_height: 0,
        diff_focus: DiffFocus::Content,
        file_pane_open: false,
        file_pane_state: ListState::default(),
    };

    loop {
        terminal.draw(|frame| render(frame, &mut app))?;
        if let Event::Key(key) = event::read()? {
            match app.view {
                View::CommitList => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::Char('j') => app.list_state.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.list_state.select_previous(),
                    KeyCode::Enter => {
                        if let Some(idx) = app.list_state.selected() {
                            let oid = app.commits[idx].oid;
                            if let Ok(diff) = load_diff(&app.repo, oid) {
                                app.diff_total_lines = compute_total_lines(&diff);
                                app.diff = Some(diff);
                                app.diff_scroll = 0;
                                app.diff_focus = DiffFocus::Content;
                                app.file_pane_state.select(Some(0));
                                app.view = View::DiffView;
                            }
                        }
                    }
                    _ => {}
                },
                View::DiffView => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('f') => {
                        match (app.file_pane_open, &app.diff_focus) {
                            // Pane closed → open and focus it
                            (false, _) => {
                                app.file_pane_open = true;
                                app.diff_focus = DiffFocus::FilePane;
                            }
                            // Pane open, focus on content → re-focus pane
                            (true, DiffFocus::Content) => {
                                app.diff_focus = DiffFocus::FilePane;
                            }
                            // Pane open, focus already on pane → close it
                            (true, DiffFocus::FilePane) => {
                                app.file_pane_open = false;
                                app.diff_focus = DiffFocus::Content;
                            }
                        }
                    }
                    code => match app.diff_focus {
                        DiffFocus::FilePane => match code {
                            KeyCode::Esc | KeyCode::Left => {
                                app.diff_focus = DiffFocus::Content;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                app.file_pane_state.select_next();
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.file_pane_state.select_previous();
                            }
                            KeyCode::Enter => {
                                if let (Some(diff), Some(idx)) =
                                    (&app.diff, app.file_pane_state.selected())
                                {
                                    let target = file_start_line(diff, idx);
                                    let max_scroll = app
                                        .diff_total_lines
                                        .saturating_sub(app.diff_viewport_height);
                                    app.diff_scroll = target.min(max_scroll);
                                }
                                app.diff_focus = DiffFocus::Content;
                            }
                            _ => {}
                        },
                        DiffFocus::Content => match code {
                            KeyCode::Esc | KeyCode::Left => {
                                app.view = View::CommitList;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let max_scroll = app
                                    .diff_total_lines
                                    .saturating_sub(app.diff_viewport_height);
                                app.diff_scroll = (app.diff_scroll + 1).min(max_scroll);
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.diff_scroll = app.diff_scroll.saturating_sub(1);
                            }
                            KeyCode::Char('e') => {
                                if let Some(diff) = &mut app.diff {
                                    for file in &mut diff.files {
                                        file.collapsed = false;
                                    }
                                    app.diff_total_lines = compute_total_lines(diff);
                                }
                            }
                            _ => {}
                        },
                    },
                },
            }
        }
    }
    Ok(())
}

fn file_start_line(diff: &CommitDiff, file_idx: usize) -> usize {
    diff.files[..file_idx]
        .iter()
        .map(|f| 1 + if f.collapsed { 1 } else { f.rows.len() })
        .sum()
}

fn load_diff(repo: &gix::Repository, oid: gix::ObjectId) -> color_eyre::Result<CommitDiff> {
    let commit = repo.find_commit(oid)?;
    let new_tree = commit.tree()?;
    let parent_oid = commit.parent_ids().next().map(|pid| pid.detach());

    let mut files: Vec<FileDiff> = Vec::new();

    match parent_oid {
        Some(pid) => {
            let parent = repo.find_commit(pid)?;
            let old_tree = parent.tree()?;
            old_tree
                .changes()?
                .track_path()
                .track_rewrites(None)
                .for_each_to_obtain_tree(&new_tree, |change| {
                    process_change(change, repo, &mut files)
                })?;
        }
        None => {
            let empty = repo.empty_tree();
            empty
                .changes()?
                .track_path()
                .track_rewrites(None)
                .for_each_to_obtain_tree(&new_tree, |change| {
                    process_change(change, repo, &mut files)
                })?;
        }
    }

    Ok(CommitDiff { files })
}

fn process_change(
    change: gix::object::tree::diff::Change<'_, '_, '_>,
    repo: &gix::Repository,
    files: &mut Vec<FileDiff>,
) -> Result<gix::object::tree::diff::Action, Box<dyn std::error::Error + Send + Sync>> {
    use gix::object::tree::diff::{Action, change::Event};

    let path = change.location.to_string();
    let (old_id, new_id): (Option<gix::ObjectId>, Option<gix::ObjectId>) = match change.event {
        Event::Addition { entry_mode, id } => {
            if entry_mode.is_tree() { return Ok(Action::Continue); }
            (None, Some(id.detach()))
        }
        Event::Deletion { entry_mode, id } => {
            if entry_mode.is_tree() { return Ok(Action::Continue); }
            (Some(id.detach()), None)
        }
        Event::Modification { previous_entry_mode, previous_id, entry_mode, id } => {
            if entry_mode.is_tree() || previous_entry_mode.is_tree() {
                return Ok(Action::Continue);
            }
            (Some(previous_id.detach()), Some(id.detach()))
        }
        Event::Rewrite { .. } => return Ok(Action::Continue),
    };

    let old_bytes: Option<Vec<u8>> = old_id
        .map(|id| repo.find_object(id).map(|o| o.data.to_vec()))
        .transpose()?;
    let new_bytes: Option<Vec<u8>> = new_id
        .map(|id| repo.find_object(id).map(|o| o.data.to_vec()))
        .transpose()?;

    let is_binary = |bytes: &[u8]| bytes.contains(&0u8);
    if old_bytes.as_deref().map_or(false, is_binary)
        || new_bytes.as_deref().map_or(false, is_binary)
    {
        return Ok(Action::Continue);
    }

    let old_text = std::str::from_utf8(old_bytes.as_deref().unwrap_or_default())
        .unwrap_or("")
        .to_owned();
    let new_text = std::str::from_utf8(new_bytes.as_deref().unwrap_or_default())
        .unwrap_or("")
        .to_owned();

    files.push(compute_file_diff(path, &old_text, &new_text));
    Ok(Action::Continue)
}

fn compute_file_diff(path: String, old_text: &str, new_text: &str) -> FileDiff {
    let diff = TextDiff::from_lines(old_text, new_text);
    let mut rows: Vec<DiffLine> = Vec::new();
    let mut lines_added = 0usize;
    let mut lines_removed = 0usize;

    for group in diff.grouped_ops(3) {
        for op in &group {
            let mut pending_del: VecDeque<String> = VecDeque::new();
            for change in diff.iter_changes(op) {
                match change.tag() {
                    ChangeTag::Equal => {
                        while let Some(del) = pending_del.pop_front() {
                            rows.push(DiffLine::Changed {
                                left: Some(del),
                                right: None,
                            });
                        }
                        let text = change.value().trim_end_matches('\n').to_string();
                        rows.push(DiffLine::Context(text));
                    }
                    ChangeTag::Delete => {
                        lines_removed += 1;
                        pending_del
                            .push_back(change.value().trim_end_matches('\n').to_string());
                    }
                    ChangeTag::Insert => {
                        lines_added += 1;
                        let text = change.value().trim_end_matches('\n').to_string();
                        if let Some(del) = pending_del.pop_front() {
                            rows.push(DiffLine::Changed {
                                left: Some(del),
                                right: Some(text),
                            });
                        } else {
                            rows.push(DiffLine::Changed {
                                left: None,
                                right: Some(text),
                            });
                        }
                    }
                }
            }
            while let Some(del) = pending_del.pop_front() {
                rows.push(DiffLine::Changed {
                    left: Some(del),
                    right: None,
                });
            }
        }
    }

    let collapsed = lines_added + lines_removed > 200;
    FileDiff {
        path,
        lines_added,
        lines_removed,
        rows,
        collapsed,
    }
}

fn wrap_path(path: &str, max_width: usize) -> Vec<String> {
    if max_width < 3 {
        return vec![format!(" {}", path)];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut remaining = path;
    let mut first = true;
    while !remaining.is_empty() {
        let prefix = if first { " " } else { "  " };
        let avail = max_width.saturating_sub(prefix.len());
        if avail == 0 { break; }
        if remaining.len() <= avail {
            lines.push(format!("{}{}", prefix, remaining));
            break;
        }
        // Break at the last '/' within the available width; fall back to a hard break.
        let break_at = remaining[..avail]
            .rfind('/')
            .map(|i| i + 1)
            .unwrap_or(avail);
        lines.push(format!("{}{}", prefix, &remaining[..break_at]));
        remaining = &remaining[break_at..];
        first = false;
    }
    lines
}

fn compute_total_lines(diff: &CommitDiff) -> usize {
    diff.files
        .iter()
        .map(|f| 1 + if f.collapsed { 1 } else { f.rows.len() })
        .sum()
}

fn build_render_lines(
    diff: &CommitDiff,
    scroll: usize,
    height: usize,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let mut all_rows: Vec<(Line<'static>, Line<'static>)> = Vec::new();

    for file in &diff.files {
        let header = format!(
            " {} (+{} -{})",
            file.path, file.lines_added, file.lines_removed
        );
        let header_line: Line<'static> = Line::styled(header, Style::new().bold());
        all_rows.push((header_line.clone(), header_line));

        if file.collapsed {
            let msg = format!(
                " {} changes hidden — press 'e' to expand",
                file.lines_added + file.lines_removed
            );
            all_rows.push((Line::raw(msg.clone()), Line::raw(msg)));
        } else {
            for row in &file.rows {
                match row {
                    DiffLine::Context(text) => {
                        let line: Line<'static> = Line::raw(format!(" {}", text));
                        all_rows.push((line.clone(), line));
                    }
                    DiffLine::Changed { left, right } => {
                        let left_line: Line<'static> = match left {
                            Some(text) => Line::from(vec![
                                Span::styled("-", Style::new().fg(Color::Red)),
                                Span::styled(
                                    format!(" {}", text),
                                    Style::new().fg(Color::Red),
                                ),
                            ]),
                            None => Line::raw(""),
                        };
                        let right_line: Line<'static> = match right {
                            Some(text) => Line::from(vec![
                                Span::styled("+", Style::new().fg(Color::Green)),
                                Span::styled(
                                    format!(" {}", text),
                                    Style::new().fg(Color::Green),
                                ),
                            ]),
                            None => Line::raw(""),
                        };
                        all_rows.push((left_line, right_line));
                    }
                }
            }
        }
    }

    all_rows.into_iter().skip(scroll).take(height).unzip()
}

fn render(frame: &mut Frame, app: &mut App) {
    match app.view {
        View::CommitList => render_list(frame, app),
        View::DiffView => render_diff(frame, app),
    }
}

fn render_list(frame: &mut Frame, app: &mut App) {
    let items: Vec<ListItem> = app
        .commits
        .iter()
        .map(|c| {
            ListItem::new(format!(
                "{}  {}  {}  {}",
                c.hash, c.date, c.author, c.message
            ))
        })
        .collect();

    let list = List::new(items)
        .block(Block::bordered().title("grit — commits"))
        .highlight_symbol("> ")
        .highlight_style(Style::new().bold());

    frame.render_stateful_widget(list, frame.area(), &mut app.list_state);
}

fn render_diff(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [title_area, body_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);

    let hash = app
        .list_state
        .selected()
        .map(|i| app.commits[i].hash.as_str())
        .unwrap_or("");
    let pane_hint = if app.file_pane_open { "f: hide files" } else { "f: show files" };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!(" {} ", hash)),
            Span::styled("Esc/← return  ", Style::new().dim()),
            Span::styled(pane_hint, Style::new().dim()),
        ])),
        title_area,
    );

    let (file_area, left_area, right_area) = if app.file_pane_open {
        let areas = Layout::horizontal([
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ])
        .split(body_area);
        (Some(areas[0]), areas[1], areas[2])
    } else {
        let areas = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(body_area);
        (None, areas[0], areas[1])
    };

    let inner_height = left_area.height.saturating_sub(2) as usize;
    app.diff_viewport_height = inner_height;

    if let (Some(area), Some(diff)) = (file_area, &app.diff) {
        let focused = app.diff_focus == DiffFocus::FilePane;
        let max_width = area.width.saturating_sub(4) as usize;
        let items: Vec<ListItem> = diff
            .files
            .iter()
            .map(|f| {
                let mut text_lines: Vec<Line<'static>> = wrap_path(&f.path, max_width)
                    .into_iter()
                    .map(Line::raw)
                    .collect();
                text_lines.push(Line::styled(
                    format!("  +{} -{}", f.lines_added, f.lines_removed),
                    Style::new().dim(),
                ));
                ListItem::new(ratatui::text::Text::from(text_lines))
            })
            .collect();
        let title = if focused { "Files (j/k Enter)" } else { "Files (f)" };
        let block = Block::bordered().title(title);
        let list = List::new(items)
            .block(block)
            .highlight_symbol("> ")
            .highlight_style(Style::new().bold().fg(Color::Yellow));
        frame.render_stateful_widget(list, area, &mut app.file_pane_state);
    }

    // Diff columns
    if let Some(diff) = &app.diff {
        let (left_lines, right_lines) = build_render_lines(diff, app.diff_scroll, inner_height);
        frame.render_widget(
            Paragraph::new(left_lines).block(Block::bordered().title("Old")),
            left_area,
        );
        frame.render_widget(
            Paragraph::new(right_lines).block(Block::bordered().title("New")),
            right_area,
        );
    }
}
