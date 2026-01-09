use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame, Terminal,
};
use rusqlite::Connection;
use std::io;
use tui_textarea::TextArea;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(value_name = "DATABASE")]
    database: String,
}

struct App<'a> {
    input: TextArea<'a>,
    conn: Connection,
    results: Vec<Vec<String>>,
    headers: Vec<String>,
    status: String,
}

impl<'a> App<'a> {
    fn new(database: &str) -> Result<Self> {
        let conn = Connection::open(database).context("Failed to open database")?;
        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_placeholder_text("Enter SQL query here...");
        input.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title("SQL Query")
                .title_style(Style::default().fg(Color::Cyan)),
        );

        Ok(Self {
            input,
            conn,
            results: Vec::new(),
            headers: Vec::new(),
            status: String::from("Ready (Ctrl-Enter to run, q to quit)"),
        })
    }

    fn execute_query(&mut self) -> Result<()> {
        let sql = self.input.lines().join("\n");
        if sql.trim().is_empty() {
            self.status = String::from("Empty query");
            return Ok(());
        }

        self.status = String::from("Running query...");

        let mut stmt = self.conn.prepare(&sql).context("Failed to prepare statement")?;
        let column_names: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut results = Vec::new();
        let rows = stmt.query_map([], |row| {
            let mut row_data = Vec::new();
            for i in 0..row.as_ref().column_count() {
                let value = match row.get_ref(i) {
                    Ok(rusqlite::types::ValueRef::Null) => String::from("NULL"),
                    Ok(rusqlite::types::ValueRef::Integer(i)) => i.to_string(),
                    Ok(rusqlite::types::ValueRef::Real(f)) => f.to_string(),
                    Ok(rusqlite::types::ValueRef::Text(s)) => String::from_utf8_lossy(s).to_string(),
                    Ok(rusqlite::types::ValueRef::Blob(_)) => String::from("<BLOB>"),
                    Err(_) => String::from("<ERROR>"),
                };
                row_data.push(value);
            }
            Ok(row_data)
        });

        match rows {
            Ok(mut row_iter) => {
                while let Some(row) = row_iter.next() {
                    match row {
                        Ok(r) => results.push(r),
                        Err(e) => {
                            self.status = format!("Error reading row: {}", e);
                            return Ok(());
                        }
                    }
                }
                self.headers = column_names;
                self.results = results;
                self.status = format!("{} rows returned (Ctrl-Enter to run, q to quit)", self.results.len());
            }
            Err(e) => {
                self.status = format!("Query error: {} (Ctrl-Enter to run, q to quit)", e);
            }
        }

        Ok(())
    }
}

fn ui(f: &mut Frame, app: &App<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Length(10), Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    f.render_widget(&app.input, chunks[0]);

    let title = if app.headers.is_empty() {
        "Results (No data)"
    } else {
        "Results"
    };

    let header_style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);

    let table = Table::new(
        app.results
            .iter()
            .enumerate()
            .map(|(i, row)| {
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
    .header(
        Row::new(app.headers.iter().map(|h| Cell::from(h.as_str()))).style(header_style)
    )
    .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(table, chunks[1]);

    let status = Paragraph::new(app.status.as_str())
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    f.render_widget(status, chunks[2]);
}

fn run_app<'a>(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, mut app: App<'a>) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, &app))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Enter => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                        app.status = String::from("Running query...");
                        terminal.draw(|f| ui(f, &app))?;
                        if let Err(e) = app.execute_query() {
                            app.status = format!("Error: {}", e);
                        }
                    } else {
                        app.input.insert_newline();
                    }
                }
                KeyCode::Esc => {
                    app.input.cancel_selection();
                }
                _ => {
                    app.input.input(key);
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(&cli.database).context("Failed to initialize app")?;

    let res = run_app(&mut terminal, app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res?;
    Ok(())
}
