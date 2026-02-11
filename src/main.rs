use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, Lines, SyntaxHighlighter,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionKind {
    Keyword,
    Table,
    Column,
}

struct AutocompleteState {
    suggestions: Vec<String>,
    selected: usize,
    visible: bool,
}

struct Schema {
    tables: Vec<String>,
    columns: Vec<String>,
    columns_by_table: std::collections::HashMap<String, Vec<String>>,
}

struct TablePickerState {
    visible: bool,
    filter: String,
    selected: usize,
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
    schema: Schema,
    focus: Pane,
    query_history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
    history_path: PathBuf,
    table_picker: TablePickerState,
}

impl App {
    fn new(database: &str) -> Result<Self> {
        let conn = Connection::open(database).context("Failed to open database")?;

        let mut editor_state = EditorState::default();
        editor_state.mode = EditorMode::Insert;
        let event_handler = EditorEventHandler::default();

        let schema = Self::load_schema(&conn)?;
        let resolved_database_path = resolve_database_path(database)?;
        let history_path = history_file_path_for_database(&resolved_database_path)?;
        let query_history = load_query_history(&history_path)?;

        let mut app = Self {
            editor_state,
            event_handler,
            database_path: resolved_database_path.to_string_lossy().to_string(),
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
            query_history,
            history_index: None,
            history_draft: None,
            history_path,
            table_picker: TablePickerState { visible: false, filter: String::new(), selected: 0 },
        };

        if let Some(last_query) = app.query_history.last().cloned() {
            app.set_query(&last_query);
            app.status = String::from("Loaded latest query from history");
        }

        Ok(app)
    }

    fn load_schema(conn: &Connection) -> Result<Schema> {
        let mut tables = Vec::new();
        let mut columns = Vec::new();
        let mut columns_by_table = std::collections::HashMap::<String, Vec<String>>::new();

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .context("Failed to query tables")?;
        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .context("Failed to fetch tables")?
            .filter_map(Result::ok)
            .collect();

        for table in &table_names {
            tables.push(table.clone());

            if let Ok(mut col_stmt) = conn.prepare(&format!("PRAGMA table_info({})", table)) {
                let table_columns: Vec<String> =
                    match col_stmt.query_map([], |row| row.get::<_, String>(1)) {
                        Ok(rows) => rows.filter_map(Result::ok).collect(),
                        Err(_) => Vec::new(),
                    };
                columns.extend(table_columns.iter().cloned());
                columns_by_table.insert(table.to_lowercase(), table_columns);
            }
        }

        tables.sort();
        tables.dedup();
        columns.sort();
        columns.dedup();

        Ok(Schema { tables, columns, columns_by_table })
    }

    fn update_autocomplete(&mut self) {
        if !matches!(self.editor_state.mode, EditorMode::Insert) {
            self.autocomplete.visible = false;
            return;
        }

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

        let before_text = text_before_cursor(&text, line, before_cursor);
        let statement_before =
            before_text.rsplit_once(';').map(|(_, s)| s).unwrap_or(before_text.as_str());
        let kind = completion_kind(statement_before);
        let qualifier = qualifier_before_word(before_cursor, word_start);

        let min_prefix_len = match kind {
            CompletionKind::Table => 0,
            CompletionKind::Column if qualifier.is_some() => 0,
            CompletionKind::Column => 0,
            CompletionKind::Keyword => 2,
        };
        if current_word.len() < min_prefix_len {
            self.autocomplete.visible = false;
            return;
        }

        let prefix_upper = current_word.to_uppercase();
        let mut suggestions = Vec::<String>::new();

        match kind {
            CompletionKind::Table => {
                suggestions.extend(self.schema.tables.iter().cloned());
            },
            CompletionKind::Column => {
                if let Some(q) = qualifier
                    && let Some(cols) = self.schema.columns_by_table.get(&q.to_lowercase())
                {
                    suggestions.extend(cols.iter().cloned());
                } else {
                    suggestions.extend(self.schema.columns.iter().cloned());
                }
            },
            CompletionKind::Keyword => {
                suggestions.extend(SQL_KEYWORDS.iter().map(|&s| s.to_string()));
            },
        }

        if !prefix_upper.is_empty() {
            suggestions.retain(|s| s.to_uppercase().starts_with(&prefix_upper));
        }
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

    fn current_query(&self) -> String {
        self.editor_state.lines.to_string()
    }

    fn set_query(&mut self, query: &str) {
        self.editor_state.lines = Lines::from(query);
        self.editor_state.selection = None;
        let last_row = self.editor_state.lines.len().saturating_sub(1);
        let last_col = self.editor_state.lines.len_col(last_row).unwrap_or_default();
        self.editor_state.cursor.row = last_row;
        self.editor_state.cursor.col = last_col;
    }

    fn history_len(&self) -> usize {
        self.query_history.len() + usize::from(self.history_draft.is_some())
    }

    fn history_entry(&self, index: usize) -> Option<&str> {
        if index < self.query_history.len() {
            return self.query_history.get(index).map(String::as_str);
        }
        if index == self.query_history.len() {
            return self.history_draft.as_deref();
        }
        None
    }

    fn ensure_history_draft(&mut self) {
        if self.history_draft.is_some() {
            return;
        }
        let current = self.current_query();
        let last_run = self.query_history.last().map(String::as_str).unwrap_or("");
        if current != last_run {
            self.history_draft = Some(current);
        }
    }

    fn history_prev(&mut self) {
        self.ensure_history_draft();
        let len = self.history_len();
        if len == 0 {
            return;
        }

        let next_index = match self.history_index {
            Some(i) if i > 0 => i - 1,
            Some(_) => 0,
            None => self.query_history.len().saturating_sub(1),
        };
        self.history_index = Some(next_index);
        if let Some(entry) = self.history_entry(next_index).map(ToString::to_string) {
            self.set_query(&entry);
        }
    }

    fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };

        self.ensure_history_draft();
        let len = self.history_len();
        if len == 0 {
            self.history_index = None;
            return;
        }

        if index + 1 >= len {
            self.history_index = None;
            if let Some(draft) = self.history_draft.clone() {
                self.set_query(&draft);
            }
            return;
        }

        let next_index = index + 1;
        self.history_index = Some(next_index);
        if let Some(entry) = self.history_entry(next_index).map(ToString::to_string) {
            self.set_query(&entry);
        }
    }

    fn append_run_query_to_history(&mut self, query: &str) {
        if query.trim().is_empty() {
            return;
        }
        if self.query_history.last().is_some_and(|last| last == query) {
            return;
        }
        self.query_history.push(query.to_string());
        self.history_index = None;
        self.history_draft = None;
        if let Err(e) = save_query_history(&self.history_path, &self.query_history) {
            self.status = format!("Warning: failed to save history: {}", e);
        }
    }

    fn save_current_query_on_exit(&mut self) {
        let query = self.current_query();
        if query.trim().is_empty() {
            return;
        }
        if self.query_history.last().is_some_and(|q| q == &query) {
            return;
        }
        self.append_run_query_to_history(&query);
    }

    fn new_query(&mut self) {
        let current = self.current_query();
        self.append_run_query_to_history(&current);
        self.set_query("");
        self.autocomplete.visible = false;
        self.status = String::from("New query");
    }

    fn filtered_tables(&self) -> Vec<String> {
        let filter = self.table_picker.filter.to_lowercase();
        self.schema
            .tables
            .iter()
            .filter(|t| filter.is_empty() || t.to_lowercase().contains(&filter))
            .cloned()
            .collect()
    }

    fn open_table_picker(&mut self) {
        self.table_picker.visible = true;
        self.table_picker.filter.clear();
        self.table_picker.selected = 0;
        self.status = String::from("Table picker: type to filter, Enter to select");
    }

    fn close_table_picker(&mut self) {
        self.table_picker.visible = false;
        self.table_picker.filter.clear();
        self.table_picker.selected = 0;
    }

    fn table_picker_move_up(&mut self) {
        self.table_picker.selected = self.table_picker.selected.saturating_sub(1);
    }

    fn table_picker_move_down(&mut self) {
        let len = self.filtered_tables().len();
        if len == 0 {
            self.table_picker.selected = 0;
            return;
        }
        self.table_picker.selected = (self.table_picker.selected + 1).min(len - 1);
    }

    fn table_picker_push_filter(&mut self, ch: char) {
        self.table_picker.filter.push(ch);
        self.table_picker.selected = 0;
    }

    fn table_picker_pop_filter(&mut self) {
        self.table_picker.filter.pop();
        self.table_picker.selected = 0;
    }

    fn table_picker_apply_selection(&mut self) -> bool {
        let tables = self.filtered_tables();
        if tables.is_empty() {
            return false;
        }
        let idx = self.table_picker.selected.min(tables.len() - 1);
        let table = tables[idx].clone();
        let columns =
            self.schema.columns_by_table.get(&table.to_lowercase()).cloned().unwrap_or_default();
        let select_clause = if columns.is_empty() { "*".to_string() } else { columns.join(", ") };
        let query = format!("select {} from {} limit 100;", select_clause, table);
        self.set_query(&query);
        self.close_table_picker();
        self.status = format!("Loaded table query: {}", table);
        true
    }

    fn handle_table_picker_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.close_table_picker(),
            KeyCode::Enter => {
                return self.table_picker_apply_selection();
            },
            KeyCode::Up => self.table_picker_move_up(),
            KeyCode::Down => self.table_picker_move_down(),
            KeyCode::Backspace => self.table_picker_pop_filter(),
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.table_picker_push_filter(ch);
            },
            _ => {},
        }
        false
    }

    fn accept_autocomplete(&mut self) {
        if !matches!(self.editor_state.mode, EditorMode::Insert) {
            self.autocomplete.visible = false;
            return;
        }

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
        self.append_run_query_to_history(&sql);

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

fn history_root_dir() -> Result<PathBuf> {
    if let Ok(dir) = env::var("SQUEAL_CONFIG_DIR") {
        return Ok(Path::new(&dir).to_path_buf());
    }
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        return Ok(Path::new(&xdg).join("squeal"));
    }
    let home = env::var("HOME").context("HOME not set")?;
    Ok(Path::new(&home).join(".config").join("squeal"))
}

fn resolve_database_path(database: &str) -> Result<PathBuf> {
    let path = Path::new(database);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(env::current_dir().context("Failed to read current directory")?.join(path))
}

fn history_file_path_for_database(database_path: &Path) -> Result<PathBuf> {
    let root = history_root_dir()?;
    let history_dir = root.join("history-by-db");
    let candidates = history_file_candidates(&history_dir, database_path);
    if let Some(existing) = candidates.iter().find(|p| p.exists()) {
        return Ok(existing.clone());
    }
    Ok(candidates
        .first()
        .cloned()
        .unwrap_or_else(|| history_file_path_with_key(&history_dir, database_path)))
}

fn history_file_candidates(history_dir: &Path, database_path: &Path) -> Vec<PathBuf> {
    let mut keys = Vec::<PathBuf>::new();

    if let Ok(canonical) = fs::canonicalize(database_path) {
        keys.push(canonical);
    }
    keys.push(database_path.to_path_buf());

    let mut files = Vec::new();
    for key in keys {
        let path = history_file_path_with_key(history_dir, &key);
        if !files.iter().any(|p: &PathBuf| p == &path) {
            files.push(path);
        }
    }
    files
}

fn history_file_path_with_key(history_dir: &Path, database_path: &Path) -> PathBuf {
    let db_key = database_path.to_string_lossy();
    let hash = stable_hash64(db_key.as_bytes());
    let name = sanitize_history_name(
        database_path.file_name().and_then(|s| s.to_str()).unwrap_or("database"),
    );
    history_dir.join(format!("{}-{:016x}.history", name, hash))
}

fn sanitize_history_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { String::from("database") } else { out }
}

fn stable_hash64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn load_query_history(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(bytes
        .split(|b| *b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect())
}

fn save_query_history(path: &Path, history: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let data = history.join("\0");
    fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn completion_kind(statement_before: &str) -> CompletionKind {
    let words = uppercase_words(statement_before);
    let mut kind = CompletionKind::Keyword;
    for w in words {
        match w.as_str() {
            "SELECT" => kind = CompletionKind::Column,
            "FROM" | "JOIN" | "INTO" | "UPDATE" => kind = CompletionKind::Table,
            "ON" => kind = CompletionKind::Column,
            "WHERE" | "GROUP" | "ORDER" | "HAVING" | "LIMIT" => {
                kind = CompletionKind::Keyword;
            },
            _ => {},
        }
    }
    kind
}

fn uppercase_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch.to_ascii_uppercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn text_before_cursor(text: &str, line: usize, before_cursor: &str) -> String {
    let mut out = String::new();
    for (i, l) in text.lines().enumerate() {
        if i < line {
            out.push_str(l);
            out.push('\n');
        } else if i == line {
            out.push_str(before_cursor);
            break;
        } else {
            break;
        }
    }
    out
}

fn qualifier_before_word(before_cursor: &str, word_start: usize) -> Option<String> {
    if word_start == 0 {
        return None;
    }
    let prefix = &before_cursor[..word_start];
    let prefix = prefix.strip_suffix('.')?;
    let q_start =
        prefix.rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_').map(|i| i + 1).unwrap_or(0);
    let q = prefix[q_start..].trim();
    if q.is_empty() { None } else { Some(q.to_string()) }
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

    if matches!(app.editor_state.mode, EditorMode::Insert)
        && app.autocomplete.visible
        && !app.autocomplete.suggestions.is_empty()
    {
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

    if matches!(app.editor_state.mode, EditorMode::Normal) && app.table_picker.visible {
        let tables = app.filtered_tables();
        let area = f.area();
        let width: u16 = 56;
        let height: u16 = 16;
        let popup_width = width.min(area.width.saturating_sub(2));
        let popup_height = height.min(area.height.saturating_sub(2));
        let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
        let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
        let popup = Rect::new(popup_x, popup_y, popup_width, popup_height);

        f.render_widget(Clear, popup);
        let block = Block::default().borders(Borders::ALL).title("Tables");
        f.render_widget(block, popup);

        let inner = Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        );
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        let filter = Paragraph::new(format!("Filter: {}", app.table_picker.filter))
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(filter, sections[0]);

        let items: Vec<ListItem> = if tables.is_empty() {
            vec![ListItem::new("<no tables>").style(Style::default().fg(Color::DarkGray))]
        } else {
            tables
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let style = if i == app.table_picker.selected {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(t.as_str()).style(style)
                })
                .collect()
        };
        f.render_widget(List::new(items), sections[1]);
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
                    if matches!(app.editor_state.mode, EditorMode::Insert)
                        && key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.save_current_query_on_exit();
                        return Ok(());
                    }
                    if matches!(app.editor_state.mode, EditorMode::Normal)
                        && key.code == KeyCode::Char('q')
                        && key.modifiers.is_empty()
                    {
                        app.save_current_query_on_exit();
                        return Ok(());
                    }
                    if matches!(app.editor_state.mode, EditorMode::Normal)
                        && app.table_picker.visible
                    {
                        if app.handle_table_picker_key(key) {
                            app.status = String::from("Running query...");
                            if let Err(e) = app.execute_query().await {
                                app.status = format!("Error: {}", e);
                            }
                        }
                        continue;
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
                                if app.focus == Pane::Editor {
                                    app.history_prev();
                                } else if app.focus == Pane::Results {
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
                                if app.focus == Pane::Editor {
                                    app.history_next();
                                } else if app.focus == Pane::Results {
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
                            KeyCode::Char('h') => {
                                if app.focus == Pane::Editor {
                                    app.history_prev();
                                } else {
                                    app.event_handler.on_key_event(key, &mut app.editor_state);
                                }
                            },
                            KeyCode::Char('l') => {
                                if app.focus == Pane::Editor {
                                    app.history_next();
                                } else {
                                    app.event_handler.on_key_event(key, &mut app.editor_state);
                                }
                            },
                            KeyCode::Char('n') => {
                                if app.focus == Pane::Editor {
                                    app.new_query();
                                } else {
                                    app.event_handler.on_key_event(key, &mut app.editor_state);
                                }
                            },
                            KeyCode::Char('t') => {
                                app.open_table_picker();
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
                        } else if key.code == KeyCode::Left && app.focus == Pane::Editor {
                            app.history_prev();
                        } else if key.code == KeyCode::Right && app.focus == Pane::Editor {
                            app.history_next();
                        } else if key.code == KeyCode::Char('h') && app.focus == Pane::Editor {
                            app.history_prev();
                        } else if key.code == KeyCode::Char('l') && app.focus == Pane::Editor {
                            app.history_next();
                        } else if key.code == KeyCode::Char('n') && app.focus == Pane::Editor {
                            app.new_query();
                        } else if key.code == KeyCode::Char('t') {
                            app.open_table_picker();
                        } else {
                            app.event_handler.on_key_event(key, &mut app.editor_state);
                        }
                    } else {
                        if matches!(app.editor_state.mode, EditorMode::Insert)
                            && (key.code == KeyCode::Tab || key.code == KeyCode::Enter)
                            && app.autocomplete.visible
                        {
                            app.accept_autocomplete();
                        } else if matches!(app.editor_state.mode, EditorMode::Insert)
                            && key.code == KeyCode::Esc
                            && app.autocomplete.visible
                        {
                            app.autocomplete.visible = false;
                        } else if matches!(app.editor_state.mode, EditorMode::Insert)
                            && key.code == KeyCode::Down
                            && app.autocomplete.visible
                        {
                            app.autocomplete.selected = (app.autocomplete.selected + 1)
                                .min(app.autocomplete.suggestions.len().saturating_sub(1));
                        } else if matches!(app.editor_state.mode, EditorMode::Insert)
                            && key.code == KeyCode::Up
                            && app.autocomplete.visible
                        {
                            app.autocomplete.selected = app.autocomplete.selected.saturating_sub(1);
                        } else {
                            app.event_handler.on_key_event(key, &mut app.editor_state);
                            app.history_index = None;
                            app.history_draft = None;
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
