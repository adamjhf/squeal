# squeal

A keyboard-first SQLite TUI for writing queries, browsing results, and iterating quickly.

## Highlights

- vim-style editing modes (`insert` / `normal`) via `edtui`
- SQL syntax highlighting (`one-dark`)
- schema-aware autocomplete in insert mode
  - table suggestions after `from`/`join`/`into`/`update`
  - column suggestions after `select` / `on`
  - supports `table.column` completion
- fixed-size table picker (`t` in normal mode)
  - type-to-filter tables
  - select table -> generates `select col1, col2, ... from table limit 100;`
  - auto-runs selected query
- per-database query history
  - keyed by sqlite file path
  - latest query auto-loaded on startup
  - avoids consecutive duplicates
- clear status/error messaging for SQL syntax/parse/table/column failures
- consistent subtle TUI palette with inline key hints

## Keybindings

### Global

- `ctrl+q` in insert mode: quit
- `q` in normal mode: quit (saves current query to history if needed)
- `tab` in normal mode: switch focus between query/results panes

### Insert mode

- `esc`: go to normal mode
- `tab` or `enter`: accept selected autocomplete suggestion
- `up` / `down`: navigate autocomplete list
- `esc` when autocomplete visible: close autocomplete popup (first press)

### Normal mode (editor focused)

- `enter`: run query
- `left` / `right` or `h` / `l`: previous/next query history
- `n`: start new query (stores current query to history if non-empty)
- `t`: open table picker

### Table picker

- type characters: filter table list
- `backspace`: delete filter char
- `up` / `down`: move selection
- `enter`: apply table query and execute
- `esc`: close picker

## Query history

History is stored per database under config dir:

- `$SQUEAL_CONFIG_DIR/history-by-db/` if `SQUEAL_CONFIG_DIR` is set
- otherwise `$XDG_CONFIG_HOME/squeal/history-by-db/`
- otherwise `~/.config/squeal/history-by-db/`

Files use a simple NUL-separated query format.

## Build and run

Run:

```bash
cargo run -- path/to/database.sqlite
```

Common checks:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Implementation notes

- single-binary app in `src/main.rs`
- async event loop with `crossterm::EventStream` + `tokio`
- blocking sqlite work offloaded with `tokio::task::spawn_blocking`
- UI built with `ratatui`
