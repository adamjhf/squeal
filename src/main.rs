use std::io;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use edtui::{EditorEventHandler, EditorState, EditorView, SyntaxHighlighter};
use futures::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    prelude::Widget,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};
use rusqlite::Connection;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(value_name = "DATABASE")]
    database: String,
}

struct App {
    editor_state: EditorState,
    event_handler: EditorEventHandler,
    database_path: String,
    results: Vec<Vec<String>>,
    headers: Vec<String>,
    status: String,
}

impl App {
    fn new(database: &str) -> Result<Self> {
        Connection::open(database).context("Failed to open database")?;

        let editor_state = EditorState::default();
        let event_handler = EditorEventHandler::default();

        Ok(Self {
            editor_state,
            event_handler,
            database_path: database.to_string(),
            results: Vec::new(),
            headers: Vec::new(),
            status: String::from("Ready (Ctrl-Enter to run, Ctrl-q to quit)"),
        })
    }

    async fn execute_query(&mut self) -> Result<()> {
        let sql = self.editor_state.lines.to_string();
        if sql.trim().is_empty() {
            self.status = String::from("Empty query");
            return Ok(());
        }

        let db_path = self.database_path.clone();

        let result =
            tokio::task::spawn_blocking(move || -> Result<(Vec<String>, Vec<Vec<String>>)> {
                let conn = Connection::open(&db_path)
                    .context("Failed to open database in background task")?;

                let mut stmt = conn.prepare(&sql).context("Failed to prepare statement")?;
                let column_names: Vec<String> =
                    stmt.column_names().iter().map(|s| s.to_string()).collect();

                let mut results = Vec::new();
                let rows = stmt.query_map([], |row| {
                    let mut row_data = Vec::new();
                    for i in 0..row.as_ref().column_count() {
                        let value = match row.get_ref(i) {
                            Ok(rusqlite::types::ValueRef::Null) => String::from("NULL"),
                            Ok(rusqlite::types::ValueRef::Integer(i)) => i.to_string(),
                            Ok(rusqlite::types::ValueRef::Real(f)) => f.to_string(),
                            Ok(rusqlite::types::ValueRef::Text(s)) => {
                                String::from_utf8_lossy(s).to_string()
                            },
                            Ok(rusqlite::types::ValueRef::Blob(_)) => String::from("<BLOB>"),
                            Err(_) => String::from("<ERROR>"),
                        };
                        row_data.push(value);
                    }
                    Ok(row_data)
                });

                match rows {
                    Ok(mut row_iter) => {
                        for row in row_iter.by_ref() {
                            results.push(row.context("Error reading row")?);
                        }
                        Ok((column_names, results))
                    },
                    Err(e) => Err(anyhow::anyhow!("Query error: {}", e)),
                }
            })
            .await
            .context("Failed to execute background task")??;

        self.headers = result.0;
        self.results = result.1;
        self.status =
            format!("{} rows returned (Ctrl-Enter to run, Ctrl-q to quit)", self.results.len());

        Ok(())
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Length(10), Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    let syntax_highlighter = SyntaxHighlighter::new("dracula", "sql").ok();
    EditorView::new(&mut app.editor_state)
        .syntax_highlighter(syntax_highlighter)
        .render(chunks[0], f.buffer_mut());

    let title = if app.headers.is_empty() { "Results (No data)" } else { "Results" };

    let header_style = Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD);

    let table = Table::new(
        app.results.iter().enumerate().map(|(i, row)| {
            let style = if i % 2 == 0 {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Rgb(150, 150, 150))
            };
            Row::new(row.iter().map(|cell| Cell::from(cell.as_str()))).style(style)
        }),
        &[
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Length(20),
        ],
    )
    .header(Row::new(app.headers.iter().map(|h| Cell::from(h.as_str()))).style(header_style))
    .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(table, chunks[1]);

    let status = Paragraph::new(app.status.as_str())
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    f.render_widget(status, chunks[2]);
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
) -> Result<()> {
    let mut event_reader = EventStream::new();

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        if let Some(Ok(event)) = event_reader.next().await {
            match event {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    },
                    KeyCode::Enter
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            || key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        app.status = String::from("Running query...");
                        if let Err(e) = app.execute_query().await {
                            app.status = format!("Error: {}", e);
                        }
                    },
                    _ => {
                        app.event_handler.on_key_event(key, &mut app.editor_state);
                    },
                },
                Event::Mouse(mouse_event) => {
                    app.event_handler.on_mouse_event(mouse_event, &mut app.editor_state);
                },
                Event::Resize(_, _) => {},
                _ => {},
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(&cli.database).context("Failed to initialize app")?;

    let res = run_app(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    res?;
    Ok(())
}
