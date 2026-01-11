use std::io;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use edtui::{EditorEventHandler, EditorMode, EditorState, EditorView, SyntaxHighlighter};
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
    current_row: usize,
    current_col: usize,
    vertical_scroll: usize,
    horizontal_scroll: usize,
    visible_rows: usize,
    visible_cols: usize,
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
            status: String::from("Ready (Enter in Normal mode to run query, Ctrl-q to quit)"),
            current_row: 0,
            current_col: 0,
            vertical_scroll: 0,
            horizontal_scroll: 0,
            visible_rows: 10,
            visible_cols: 5,
        })
    }

    async fn execute_query(&mut self) -> Result<()> {
        let sql = self.editor_state.lines.to_string();
        if sql.trim().is_empty() {
            self.status = String::from("Empty query");
            return Ok(());
        }

        let statements: Vec<String> =
            sql.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if statements.is_empty() {
            self.status = String::from("Empty query");
            return Ok(());
        }

        let db_path = self.database_path.clone();

        let result =
            tokio::task::spawn_blocking(move || -> Result<(Vec<String>, Vec<Vec<String>>)> {
                let conn = Connection::open(&db_path)
                    .context("Failed to open database in background task")?;

                // Execute all statements except the last one
                for stmt_sql in &statements[..statements.len() - 1] {
                    let mut stmt = conn
                        .prepare(stmt_sql)
                        .context(format!("Failed to prepare statement: {}", stmt_sql))?;
                    if stmt.column_count() > 0 {
                        // SELECT-like statement: execute but discard results
                        let _ = stmt
                            .query_map([], |_| Ok(()))
                            .context(format!("Failed to execute query: {}", stmt_sql))?;
                    } else {
                        // Non-SELECT statement: use execute
                        conn.execute(stmt_sql, [])
                            .context(format!("Failed to execute statement: {}", stmt_sql))?;
                    }
                }

                // Prepare and execute the last statement to get results
                let last_sql = &statements[statements.len() - 1];
                let mut stmt =
                    conn.prepare(last_sql).context("Failed to prepare last statement")?;
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
        self.current_row = 0;
        self.current_col = 0;
        self.vertical_scroll = 0;
        self.horizontal_scroll = 0;
        self.status = format!(
            "{} rows returned (Enter in Normal mode to run query, Ctrl-q to quit)",
            self.results.len()
        );

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

    app.visible_rows = (chunks[1].height as usize).saturating_sub(3);
    app.visible_cols = (chunks[1].width / 10).max(1) as usize;

    let results_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(chunks[1]);

    let title = if app.headers.is_empty() { "Results (No data)" } else { "Results" };

    let header_style = Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD);

    let constraints: Vec<Constraint> = app.headers.iter().map(|_| Constraint::Min(10)).collect();

    let start_row = app.vertical_scroll;
    let end_row = (start_row + app.visible_rows).min(app.results.len());

    let table = Table::new(
        app.results[start_row..end_row].iter().enumerate().map(|(i, row)| {
            let global_i = i + start_row;
            Row::new(row.iter().enumerate().map(|(j, cell)| {
                let base_style = if global_i % 2 == 0 {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Rgb(150, 150, 150))
                };
                let mut cell = Cell::from(cell.as_str()).style(base_style);
                if global_i == app.current_row && j == app.current_col {
                    cell = cell.style(Style::default().fg(Color::Black).bg(Color::White));
                }
                cell
            }))
        }),
        constraints,
    )
    .header(Row::new(app.headers.iter().map(|h| Cell::from(h.as_str()))).style(header_style))
    .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(table, results_area[0]);

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
                    KeyCode::Enter if matches!(app.editor_state.mode, EditorMode::Normal) => {
                        app.status = String::from("Running query...");
                        if let Err(e) = app.execute_query().await {
                            app.status = format!("Error: {}", e);
                        }
                    },
                    _ => {
                        if matches!(app.editor_state.mode, EditorMode::Normal)
                            && !app.results.is_empty()
                        {
                            match key.code {
                                KeyCode::Up => {
                                    if app.current_row > 0 {
                                        app.current_row -= 1;
                                        if app.current_row < app.vertical_scroll {
                                            app.vertical_scroll = app.current_row;
                                        }
                                    }
                                },
                                KeyCode::Down => {
                                    if app.current_row + 1 < app.results.len() {
                                        app.current_row += 1;
                                        if app.current_row >= app.vertical_scroll + app.visible_rows
                                        {
                                            app.vertical_scroll =
                                                app.current_row - app.visible_rows + 1;
                                        }
                                    }
                                },
                                KeyCode::Left => {
                                    if app.current_col > 0 {
                                        app.current_col -= 1;
                                        if app.current_col < app.horizontal_scroll {
                                            app.horizontal_scroll = app.current_col;
                                        }
                                    }
                                },
                                KeyCode::Right => {
                                    if app.current_col + 1 < app.headers.len() {
                                        app.current_col += 1;
                                        if app.current_col
                                            >= app.horizontal_scroll + app.visible_cols
                                        {
                                            app.horizontal_scroll =
                                                app.current_col - app.visible_cols + 1;
                                        }
                                    }
                                },
                                _ => {
                                    app.event_handler.on_key_event(key, &mut app.editor_state);
                                },
                            }
                        } else {
                            app.event_handler.on_key_event(key, &mut app.editor_state);
                        }
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
