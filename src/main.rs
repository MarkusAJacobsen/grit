use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    DefaultTerminal, Frame,
    style::Style,
    widgets::{Block, List, ListItem, ListState},
};

struct CommitInfo {
    hash: String,
    author: String,
    date: String,
    message: String,
}

struct App {
    commits: Vec<CommitInfo>,
    list_state: ListState,
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
                msg.split(|&b| b == b'\n').next().unwrap_or_default()
            ).into_owned();
            let dt = chrono::DateTime::from_timestamp(author.time.seconds, 0)?;
            Some(CommitInfo {
                hash,
                author: String::from_utf8_lossy(author.name).into_owned(),
                date: dt.format("%Y-%m-%d").to_string(),
                message: first_line,
            })
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let mut app = App { commits, list_state };

    loop {
        terminal.draw(|frame| render(frame, &mut app))?;
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => app.list_state.select_next(),
                KeyCode::Up | KeyCode::Char('k') => app.list_state.select_previous(),
                _ => {}
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame, app: &mut App) {
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
