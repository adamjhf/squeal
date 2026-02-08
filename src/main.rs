use std::io;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, SyntaxHighlighter,
};
use futures::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::Widget,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
};
use rusqlite::Connection;

const SQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "UPDATE",
    "DELETE",
    "CREATE",
    "DROP",
    "TABLE",
    "INDEX",
    "VIEW",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "ON",
    "AS",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "EXISTS",
    "BETWEEN",
    "LIKE",
    "LIMIT",
    "ORDER",
    "BY",
    "GROUP",
    "HAVING",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "DISTINCT",
    "ASC",
    "DESC",
    "VALUES",
    "INTO",
    "SET",
    "ALTER",
    "ADD",
    "COLUMN",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "UNIQUE",
    "DEFAULT",
    "AUTOINCREMENT",
    "IF",
    "ELSE",
    "CASE",
    "WHEN",
    "THEN",
    "END",
    "CAST",
    "COALESCE",
    "LENGTH",
    "SUBSTR",
    "UPPER",
    "LOWER",
    "TRIM",
    "REPLACE",
    "ROUND",
    "ABS",
    "RANDOM",
    "DATE",
    "TIME",
    "DATETIME",
    "JULIANDAY",
    "STRFTIME",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "PRAGMA",
    "EXPLAIN",
    "QUERY",
    "PLAN",
    "VACUUM",
    "ANALYZE",
    "ATTACH",
    "DETACH",
    "REINDEX",
];

struct AutocompleteState {
    suggestions: Vec<String>,
    selected: usize,
    visible: bool,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(value_name = "DATABASE")]
    database: String,
}

#[derive(PartialEq)]
enum Pane {
    Editor,
    Results,
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
    autocomplete: AutocompleteState,
    schema: Vec<String>,
    focus: Pane,
}

impl App {
    fn new(database: &str) -> Result<Self> {
        let conn = Connection::open(database).context("Failed to open database")?;

        let mut editor_state = EditorState::default();
        editor_state.mode = EditorMode::Insert;
        let event_handler = EditorEventHandler::default();

        let schema = Self::load_schema(&conn)?;

        Ok(Self {
            editor_state,
            event_handler,
            database_path: database.to_string(),
            results: Vec::new(),
            headers: Vec::new(),
            status: String::from(
                "Ready (Ctrl+Enter to run query, Tab to switch focus, Ctrl+q to quit)",
            ),
            current_row: 0,
            current_col: 0,
            vertical_scroll: 0,
            horizontal_scroll: 0,
            visible_rows: 10,
            visible_cols: 5,
            autocomplete: AutocompleteState {
                suggestions: Vec::new(),
                selected: 0,
                visible: false,
            },
            schema,
            focus: Pane::Editor,
        })
    }

    fn load_schema(conn: &Connection) -> Result<Vec<String>> {
        let mut schema = Vec::new();

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .context("Failed to query tables")?;
        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .context("Failed to fetch tables")?
            .filter_map(Result::ok)
            .collect();

        for table in &table_names {
            schema.push(table.clone());

            if let Ok(mut col_stmt) = conn.prepare(&format!("PRAGMA table_info({})", table)) {
                let columns: Vec<String> =
                    match col_stmt.query_map([], |row| row.get::<_, String>(1)) {
                        Ok(rows) => rows.filter_map(Result::ok).collect(),
                        Err(_) => Vec::new(),
                    };
                schema.extend(columns);
            }
        }

        schema.sort();
        schema.dedup();
        Ok(schema)
    }

    fn update_autocomplete(&mut self) {
        let text = self.editor_state.lines.to_string();
        let cursor = &self.editor_state.cursor;
        let line = cursor.row;
        let col = cursor.col;

        if line >= text.lines().count() {
            self.autocomplete.visible = false;
            return;
        }

        let current_line = text.lines().nth(line).unwrap_or("");
        let before_cursor = &current_line[..col.min(current_line.len())];

        let word_start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let current_word = &before_cursor[word_start..];

        if current_word.len() < 2 {
            self.autocomplete.visible = false;
            return;
        }

        let prefix = current_word.to_uppercase();
        let mut suggestions: Vec<String> = SQL_KEYWORDS
            .iter()
            .filter(|&&kw| kw.starts_with(&prefix))
            .map(|&s| s.to_string())
            .collect();

        let schema_suggestions: Vec<String> =
            self.schema.iter().filter(|s| s.to_uppercase().starts_with(&prefix)).cloned().collect();

        suggestions.extend(schema_suggestions);
        suggestions.sort();
        suggestions.dedup();

        if suggestions.is_empty() {
            self.autocomplete.visible = false;
        } else {
            self.autocomplete.suggestions = suggestions;
            self.autocomplete.selected = 0;
            self.autocomplete.visible = true;
        }
    }

    fn accept_autocomplete(&mut self) {
        if !self.autocomplete.visible || self.autocomplete.suggestions.is_empty() {
            return;
        }

        let selected = self.autocomplete.selected.min(self.autocomplete.suggestions.len() - 1);
        let suggestion = &self.autocomplete.suggestions[selected];

        let cursor = &self.editor_state.cursor;
        let line = cursor.row;
        let col = cursor.col;

        let text = self.editor_state.lines.to_string();
        if line >= text.lines().count() {
            return;
        }

        let current_line = text.lines().nth(line).unwrap_or("");
        let before_cursor = &current_line[..col.min(current_line.len())];
        let word_start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);

        for _ in word_start..col {
            use crossterm::event::KeyEvent;
            self.event_handler
                .on_key_event(KeyEvent::from(KeyCode::Backspace), &mut self.editor_state);
        }

        for ch in suggestion.chars() {
            use crossterm::event::KeyEvent;
            if ch == ' ' {
                self.event_handler
                    .on_key_event(KeyEvent::from(KeyCode::Char(' ')), &mut self.editor_state);
            } else {
                self.event_handler
                    .on_key_event(KeyEvent::from(KeyCode::Char(ch)), &mut self.editor_state);
            }
        }

        self.autocomplete.visible = false;
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
        self.status =
            format!("{} rows returned (Tab to switch focus, Ctrl+q to quit)", self.results.len());

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
    let (mode_str, _mode_border_color) = match app.editor_state.mode {
        EditorMode::Insert => ("INSERT", Color::Green),
        EditorMode::Normal => ("NORMAL", Color::White),
        EditorMode::Visual => ("VISUAL", Color::Yellow),
        _ => ("", Color::White),
    };
    let focus_border_color = match app.focus {
        Pane::Editor => Color::White,
        Pane::Results => Color::Rgb(100, 100, 100),
    };
    let editor_block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Query ({}) ", mode_str))
        .border_style(Style::default().fg(focus_border_color));
    let theme = EditorTheme::default()
        .base(Style::default().bg(Color::Reset))
        .hide_status_line()
        .block(editor_block);
    EditorView::new(&mut app.editor_state)
        .syntax_highlighter(syntax_highlighter)
        .theme(theme)
        .render(chunks[0], f.buffer_mut());

    app.visible_rows = (chunks[1].height as usize).saturating_sub(3);

    let title = if app.headers.is_empty() { "Results (No data)" } else { "Results" };

    let header_style = Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD);

    // Calculate column widths: max of header and data lengths, minimum 30
    let mut widths = vec![];
    for j in 0..app.headers.len() {
        let mut max_len = app.headers[j].len();
        for row in &app.results {
            if j < row.len() {
                max_len = max_len.max(row[j].len());
            }
        }
        widths.push(max_len as u16);
    }

    let start_row = app.vertical_scroll;
    let end_row = (start_row + app.visible_rows).min(app.results.len());
    let start_col = app.horizontal_scroll;

    // Determine how many columns fit in the available width
    let available_width = chunks[1].width as usize;
    let mut cumulative = 0;
    let mut num_visible = 0;
    for &w in &widths[start_col..] {
        if cumulative + w as usize <= available_width {
            cumulative += w as usize;
            num_visible += 1;
        } else {
            break;
        }
    }
    app.visible_cols = num_visible;
    let end_col = (start_col + num_visible).min(app.headers.len());

    let headers_slice = &app.headers[start_col..end_col];
    let widths_slice = &widths[start_col..end_col];
    let constraints: Vec<Constraint> =
        widths_slice.iter().map(|&w| Constraint::Length(w)).collect();

    let table = Table::new(
        app.results[start_row..end_row].iter().enumerate().map(|(i, row)| {
            let global_i = i + start_row;
            let row_end = start_col + headers_slice.len().min(row.len().saturating_sub(start_col));
            let row_slice = &row[start_col..end_col.min(row_end)];
            Row::new(row_slice.iter().enumerate().map(|(j, cell)| {
                let local_j = j + start_col;
                let base_style = if global_i.is_multiple_of(2) {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Rgb(150, 150, 150))
                };
                let mut cell = Cell::from(cell.as_str()).style(base_style);
                if global_i == app.current_row && local_j == app.current_col {
                    cell = cell.style(Style::default().fg(Color::Black).bg(Color::White));
                }
                cell
            }))
        }),
        constraints,
    )
    .header(Row::new(headers_slice.iter().map(|h| Cell::from(h.as_str()))).style(header_style))
    .block(Block::default().borders(Borders::ALL).title(title).border_style(
        Style::default().fg(match app.focus {
            Pane::Results => Color::White,
            Pane::Editor => Color::Rgb(100, 100, 100),
        }),
    ));

    f.render_widget(table, chunks[1]);

    let status = Paragraph::new(app.status.as_str())
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    f.render_widget(status, chunks[2]);

    if app.autocomplete.visible && !app.autocomplete.suggestions.is_empty() {
        let cursor = &app.editor_state.cursor;
        let cursor_row = cursor.row as u16;
        let cursor_col = cursor.col as u16;

        let popup_width =
            app.autocomplete.suggestions.iter().map(|s| s.len()).max().unwrap_or(20).max(20) as u16;
        let popup_height = app.autocomplete.suggestions.len().min(8) as u16;

        let popup_x = chunks[0].x + cursor_col + 2;
        let popup_y = chunks[0].y + cursor_row + 2;

        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        let items: Vec<ListItem> = app
            .autocomplete
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let style = if i == app.autocomplete.selected {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default().bg(Color::Black).fg(Color::White)
                };
                ListItem::new(s.as_str()).style(style)
            })
            .collect();

        let list = List::new(items).highlight_style(Style::default().bg(Color::DarkGray));

        f.render_widget(Clear, popup_area);
        f.render_widget(list, popup_area);
    }
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
                Event::Key(key) => {
                    if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(());
                    }
                    if key.code == KeyCode::Enter
                        && matches!(app.editor_state.mode, EditorMode::Normal)
                    {
                        app.status = String::from("Running query...");
                        if let Err(e) = app.execute_query().await {
                            app.status = format!("Error: {}", e);
                        }
                    } else if matches!(app.editor_state.mode, EditorMode::Normal)
                        && !app.results.is_empty()
                    {
                        match key.code {
                            KeyCode::Up => {
                                if app.focus == Pane::Results && app.current_row > 0 {
                                    app.current_row -= 1;
                                    if app.current_row < app.vertical_scroll {
                                        app.vertical_scroll = app.current_row;
                                    }
                                }
                            },
                            KeyCode::Down => {
                                if app.focus == Pane::Results
                                    && app.current_row + 1 < app.results.len()
                                {
                                    app.current_row += 1;
                                    if app.current_row >= app.vertical_scroll + app.visible_rows {
                                        app.vertical_scroll =
                                            app.current_row - app.visible_rows + 1;
                                    }
                                }
                            },
                            KeyCode::Left => {
                                if app.focus == Pane::Results {
                                    if app.horizontal_scroll > 0
                                        && app.current_col == app.horizontal_scroll
                                    {
                                        app.horizontal_scroll -= 1;
                                        if app.current_col > 0 {
                                            app.current_col -= 1;
                                        }
                                    } else if app.current_col > app.horizontal_scroll {
                                        app.current_col -= 1;
                                    }
                                }
                            },
                            KeyCode::Right => {
                                if app.focus == Pane::Results {
                                    if app.current_col + 1
                                        == app.horizontal_scroll + app.visible_cols
                                        && app.horizontal_scroll + app.visible_cols
                                            < app.headers.len()
                                    {
                                        app.horizontal_scroll += 1;
                                    } else if app.current_col + 1 < app.headers.len() {
                                        app.current_col += 1;
                                    }
                                }
                            },
                            KeyCode::Tab => {
                                app.focus = match app.focus {
                                    Pane::Editor => Pane::Results,
                                    Pane::Results => Pane::Editor,
                                };
                            },
                            _ => {
                                app.event_handler.on_key_event(key, &mut app.editor_state);
                            },
                        }
                    } else if matches!(app.editor_state.mode, EditorMode::Normal) {
                        if key.code == KeyCode::Tab {
                            app.focus = match app.focus {
                                Pane::Editor => Pane::Results,
                                Pane::Results => Pane::Editor,
                            };
                        } else {
                            app.event_handler.on_key_event(key, &mut app.editor_state);
                        }
                    } else {
                        if key.code == KeyCode::Tab && app.autocomplete.visible {
                            app.accept_autocomplete();
                        } else if key.code == KeyCode::Down && app.autocomplete.visible {
                            app.autocomplete.selected = (app.autocomplete.selected + 1)
                                .min(app.autocomplete.suggestions.len().saturating_sub(1));
                        } else if key.code == KeyCode::Up && app.autocomplete.visible {
                            app.autocomplete.selected = app.autocomplete.selected.saturating_sub(1);
                        } else {
                            app.event_handler.on_key_event(key, &mut app.editor_state);
                            app.update_autocomplete();
                        }
                    }
                },
                Event::Mouse(mouse_event) => {
                    app.event_handler.on_mouse_event(mouse_event, &mut app.editor_state);
                    app.update_autocomplete();
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
