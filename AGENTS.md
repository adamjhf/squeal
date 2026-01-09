# Agent Guidelines for Squeal

## Project Overview

Squeal is a minimalist SQL TUI (Text User Interface), initially for SQLite with plans to expand to PostgreSQL and other databases. The design philosophy emphasizes:

- **Keyboard-driven**: Vim-inspired keybindings for efficient navigation
- **Minimalist UX**: Show only what's needed at each moment
- **Speed**: Fast startup, responsive interactions, minimal overhead
- **Focus mode**: When writing queries, show only the query editor and current results
- **Read-only results**: Result viewer is currently read-only (future: inline editing)
- **Syntax highlighting**: Support for SQL syntax highlighting
- **Autocompletion**: Tab completion for SQL keywords and schema objects
- **Zero config**: Respect user's existing shell theme and terminal settings

## Development Environment

This project uses Nix flakes with direnv for reproducible development environments.

### Initial Setup

```bash
# Enable direnv (one-time setup)
direnv allow

# This automatically loads the Nix development shell with:
# - Rust stable toolchain (cargo, rustc, clippy, rustfmt)
# - rust-analyzer for IDE support
# - All necessary build dependencies
```

Once `direnv` is enabled, the environment loads automatically when you `cd` into the project directory. All `cargo` commands run within the Nix environment.

### Manual Shell Access

```bash
# If not using direnv, manually enter the dev shell
nix develop

# Now you can run cargo commands
cargo build
cargo test
```

## Build, Lint, Test Commands

All commands below run within the Nix development environment (automatic with direnv, or manual with `nix develop`).

### Build and Run
```bash
cargo build                    # Build debug binary
cargo build --release          # Build optimized binary
cargo run -- path/to/db.sqlite # Run with database argument
cargo check                    # Fast compilation check without producing binary
```

### Linting
```bash
cargo clippy                   # Run Clippy linter
cargo clippy -- -D warnings    # Treat warnings as errors
cargo clippy --fix             # Auto-fix clippy suggestions
cargo fmt                      # Format code
cargo fmt -- --check           # Check formatting without modifying
```

**Zero warnings policy**: Always run `cargo check` after changes and fix all warnings before committing.

### Testing
```bash
cargo test                     # Run all tests
cargo test test_name           # Run specific test by name
cargo test -- --nocapture      # Show println! output
cargo test -- --test-threads=1 # Run tests serially
```

**Note**: Currently no tests exist. When adding tests, follow these patterns:
- Unit tests: Place in `#[cfg(test)]` module at bottom of file
- Integration tests: Create `tests/` directory with separate files
- Use `rusqlite::Connection::open_in_memory()` for test databases

## Code Style Guidelines

### General Principles
- Always use modern, idiomatic Rust patterns
- Leverage Rust's type system for safety and expressiveness
- Prefer functional patterns over imperative where appropriate
- Use idiomatic iterators and collection methods (map, filter, fold, etc.)
- Take advantage of Rust's ownership model for zero-cost abstractions
- **Async runtime**: Use Tokio for async execution
- **Async patterns**: Prefer async/await over blocking operations where possible
- **Zero warnings**: Always run `cargo check` after changes and fix any compilation errors or warnings before committing
- **Edition**: Rust 2021
- **Formatting**: Enforced via rustfmt
- **Linting**: Use clippy for additional code quality checks
- **Error Handling**: Use `anyhow::Result<T>` with `.context()` for error propagation
- No documentation updates unless explicitly requested
- No emoji unless explicitly requested

### Import Organization

Group imports by crate, alphabetically sorted within groups. No blank lines between groups.

```rust
use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Table},
    Frame, Terminal,
};
use rusqlite::Connection;
use std::io;
```

**Rules**:
- External crates first, then `std`
- Multi-line imports use nested braces with 4-space indentation
- No wildcard imports (`use foo::*`)
- Alphabetical order within each crate's imports

### Formatting

- **Indentation**: 4 spaces (never tabs)
- **Line length**: Keep under 100 characters where reasonable
- **Blank lines**: One blank line between functions, no blank lines between impl methods
- **Braces**: Opening brace on same line, closing brace aligned with keyword
- **Method chaining**: One method per line, indented 4 spaces

```rust
Layout::default()
    .direction(Direction::Vertical)
    .margin(1)
    .constraints([Constraint::Length(10)])
    .split(f.area());
```

### Types and Lifetimes

- Use explicit lifetime parameters when structs contain borrowed data: `struct App<'a>`
- Prefer owned types (`String`, `Vec<T>`) for application state
- Use `&str` for function parameters that don't need ownership
- Elide lifetimes in function signatures where possible: `&App<'_>`
- No type annotations on locals if compiler can infer

```rust
struct App<'a> {
    input: TextArea<'a>,       // Lifetime required
    results: Vec<Vec<String>>, // Owned data
    status: String,
}

fn ui(f: &mut Frame, app: &App<'_>) {
    // Elided lifetime
}
```

### Naming Conventions

- **Structs/Enums**: `PascalCase` (`App`, `QueryResult`)
- **Functions/Methods**: `snake_case` (`execute_query`, `run_app`)
- **Variables**: `snake_case` (`column_names`, `row_data`)
- **Constants**: `SCREAMING_SNAKE_CASE` (`DEFAULT_TIMEOUT`)
- **Lifetimes**: `'a`, `'b` (conventional short names)
- **Frame parameter**: Use `f` (ratatui convention)

### Error Handling

Use `anyhow` for error handling with contextual information. Never use `.unwrap()` or `.expect()` in production code.

```rust
// Function signatures (async where appropriate)
async fn new(database: &str) -> Result<Self>
async fn execute_query(&mut self) -> Result<()>

// Add context when propagating errors
Connection::open(database).context("Failed to open database")?;

// Handle recoverable errors gracefully (store in status, don't panic)
if let Err(e) = app.execute_query().await {
    app.status = format!("Error: {}", e);
}

// Manual pattern matching for nuanced error handling
match rows {
    Ok(data) => { /* process */ }
    Err(e) => {
        self.status = format!("Query error: {}", e);
        return Ok(());  // Recoverable, don't propagate
    }
}

// Use spawn_blocking for sync database operations
let result = tokio::task::spawn_blocking(move || {
    // Synchronous database work
    conn.execute(&sql, params)
}).await??;
```

**Rules**:
- Always use `.context()` when propagating with `?`
- UI errors should update status messages, not crash the app
- Terminal cleanup code must run even on error
- No `.unwrap()` or `.expect()` except in tests or truly infallible operations
- Wrap blocking database calls in `tokio::task::spawn_blocking` to avoid blocking async runtime

### Comments and Documentation

Use comments sparingly and only where code is non-intuitive or complex. Prefer self-documenting code over comments. Avoid redundant comments that restate what the code obviously does.

```rust
// ✗ Bad: Obvious comment
// Loop through rows
for row in rows {

// ✓ Good: Complex logic explained
// rusqlite returns ValueRef which must be converted to owned String
// for safe storage across query boundaries
let value = match row.get_ref(i) {
    Ok(ValueRef::Null) => String::from("NULL"),
    // ...
}
```

- No documentation updates unless explicitly requested
- No emoji unless explicitly requested

### Architecture Patterns

Follow the Elm-like Model-View-Update pattern with async/await:

```rust
// State: Single struct holding all app state
struct App<'a> {
    input: TextArea<'a>,       // UI widget state
    conn: Connection,          // Business logic
    results: Vec<Vec<String>>, // View model
}

// Update: Async methods mutate state
impl App {
    async fn execute_query(&mut self) -> Result<()> { }
}

// View: Pure rendering function
fn ui(f: &mut Frame, app: &App<'_>) {
    // Read-only, no mutations
}

// Event loop: Async with non-blocking event handling
async fn run_app(terminal: &mut Terminal, mut app: App) -> Result<()> {
    let mut event_reader = EventStream::new();
    
    loop {
        terminal.draw(|f| ui(f, &app))?;
        
        if let Some(Ok(Event::Key(key))) = event_reader.next().await {
            // Update app state asynchronously
        }
    }
}

// Main function with Tokio runtime
#[tokio::main]
async fn main() -> Result<()> {
    // Setup terminal
    // ...
    
    let res = run_app(&mut terminal, app).await;
    
    // Cleanup terminal
    // ...
}
```

**Patterns to follow**:
- Keep `ui()` pure (no side effects, no mutations)
- State updates happen in async event loop or async App methods
- Terminal setup/cleanup uses setup-run-cleanup pattern
- **Async event loop**: Use `crossterm::event::EventStream` with async/await for non-blocking event handling
- Database operations should be async (use `tokio::task::spawn_blocking` for sync DB calls)
- Long-running operations run asynchronously without blocking the UI

### TUI-Specific Guidelines

- Use zebra striping for table readability (alternating row colors)
- Respect terminal theme: Use semantic colors (Cyan for headers, Yellow for status)
- Fixed-height UI elements use `Constraint::Length(n)`
- Flexible elements use `Constraint::Min(0)`
- Always render status bar at bottom for user feedback
- Update status immediately before long operations: `app.status = "Running..."`

### Future Considerations

When adding features, maintain these principles:
- **Database abstraction**: Design for multi-database support (trait for Connection)
- **Syntax highlighting**: Integrate with user's theme colors
- **Autocompletion**: Query schema lazily, cache results
- **Keybindings**: Document in status bar or help overlay, follow Vim conventions
- **Performance**: Profile before optimizing, but prioritize perceived speed
- **Configuration**: Avoid config files; detect from environment where possible

## Development Workflow

1. Make changes to `src/main.rs`
2. Run `cargo fmt` to format
3. Run `cargo clippy` to check for issues
4. Test manually with a sample database
5. Commit with clear, focused message

When project grows beyond 500 lines, split into modules:
- `app.rs` - App struct and state management
- `ui.rs` - Rendering functions
- `query.rs` - SQL execution logic
- `db.rs` - Database connection abstraction
