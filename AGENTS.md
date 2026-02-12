# Agent Guidelines for Squeal

## Project overview

`squeal` is a keyboard-first SQLite TUI implemented in Rust.

Current behavior:

- editor modes: `insert`, `normal` (via `edtui`)
- results pane with row/column navigation
- schema-aware autocomplete in insert mode
- table picker modal in normal mode (`t`) with type-to-filter + auto-run
- query history persisted per database file
- latest query for current DB auto-loaded on startup
- improved SQL error messaging in status bar
- subtle, consistent one-dark-inspired UI palette with key-hint row

## Development environment

Project uses Nix flakes + direnv.

```bash
direnv allow
# or
nix develop
```

## Build/test commands

```bash
cargo check
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo run -- path/to/db.sqlite
```

Use `nix develop -c ...` when running outside direnv.

## Keybindings to preserve

Global:

- `ctrl+q` (insert): quit
- `q` (normal): quit
- `tab` (normal): switch editor/results focus

Insert mode:

- `esc`: normal mode
- autocomplete visible:
  - `tab`/`enter`: accept
  - `up`/`down`: move selection
  - `esc`: close popup (first press), then mode switch on second

Normal mode (editor focus):

- `enter`: execute query
- `left`/`right` or `h`/`l`: history prev/next
- `n`: clear editor to new query (store current query in history if non-empty)
- `t`: open table picker

Table picker modal:

- type: filter
- `backspace`: delete filter char
- `up`/`down`: selection
- `enter`: replace query with `select col1, col2, ... from table limit 100;` and run
- `esc`: close

## History model

Per-database history path:

- `$SQUEAL_CONFIG_DIR/history-by-db/` or
- `$XDG_CONFIG_HOME/squeal/history-by-db/` or
- `~/.config/squeal/history-by-db/`

Implementation details:

- DB path normalized to absolute path
- history file name includes sanitized DB filename + stable hash of DB path
- file format is NUL-separated query strings
- consecutive duplicate queries are skipped
- on startup, latest query is loaded for that DB
- on quit, current query is saved if non-empty and not already latest

## Implementation notes

- main app is in `src/main.rs`
- event loop uses `tokio` + `crossterm::event::EventStream`
- SQLite work runs in `tokio::task::spawn_blocking`
- TUI rendering via `ratatui`
- syntax highlighting via `edtui` with `one-dark`

## Editing guidance

- keep keyboard-driven UX consistent
- avoid panics: clamp popup/rect rendering bounds; avoid unsafe string byte slicing
- keep UI state transitions explicit (mode/focus/picker/autocomplete)
- keep files reasonably sized; split modules if complexity keeps growing
- preserve zero-warnings policy
