# hurl

`hurl` is a terminal UI API client for humans.

It is a Rust project that aims to feel like a lightweight, keyboard-driven Postman for the terminal, focused on creating, saving, and sending JSON-oriented HTTP requests from a TUI.

## Features

- Create a request with:
  - optional title
  - HTTP method
  - URL
  - headers
  - JSON request body
- Save requests into a local library
- Browse saved requests in the library pane
- Load and submit saved requests
- View:
  - status code
  - response time
  - response headers
  - response body
- Paste into text fields and the JSON body editor

## Tech Stack

- [`ratatui`](https://github.com/ratatui/ratatui) for layout and rendering
- [`crossterm`](https://github.com/crossterm-rs/crossterm) for terminal input/output
- [`tui-textarea`](https://github.com/rhysd/tui-textarea) for request editing
- [`reqwest`](https://github.com/seanmonstar/reqwest) + `tokio` for async HTTP
- `serde` / `serde_json` for persistence and JSON formatting

## Layout

The interface is split into three main panes:

```text
+----------------------+---------------------------------------------+
| Library              | Request                                     |
| saved requests       | title / method / url / headers / JSON body |
+----------------------+---------------------------------------------+
| Response                                                           |
| status / time / headers / body                                     |
+--------------------------------------------------------------------+
```

## Getting Started

### Prerequisites

- Rust installed locally
- A reasonably modern terminal
- macOS, Linux, or Windows

If you do not already have Rust installed, the usual path is:

```bash
curl https://sh.rustup.rs -sSf | sh
```

### Run the App

From the repo root:

```bash
cargo run
```

### Run the Tests

```bash
cargo test
```

## How To Use

### Create a Request

1. Launch the app with `cargo run`.
2. Press `n` to create a new draft.
3. Use `Up` / `Down` in the request pane to move between fields.
4. Press `Enter` to edit the selected field.
5. Press `Esc` to leave edit mode.
6. Press `Ctrl+S` to save the request to the library.

### Submit a Saved Request

1. Press `Esc` if you are currently editing.
2. Press `Tab` until focus is on `Library`.
3. Use `Up` / `Down` to highlight a saved request.
4. Press `Enter` to load it into the editor.
5. Press `Ctrl+R` to send it.

The response appears in the bottom-right `Response` pane.

### Quit the App

1. Press `Esc` if you are editing a field.
2. Press `q`.

## Keybindings

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Cycle focus between panes |
| `Up` / `Down` | Move through library items, request fields, or response scroll |
| `Enter` | Load a library item or enter edit mode for the selected request field |
| `Esc` | Leave edit mode |
| `n` | Create a new request draft |
| `Ctrl+V` | Paste from clipboard into the active request text field |
| `Ctrl+S` | Save the current request |
| `Ctrl+R` | Send the current request |
| `q` | Quit |

## Paste Behavior

- For `Title` and `URL`, pasted newlines are flattened into spaces.
- For `Headers` and `JSON Body`, multiline paste is preserved.
- You need to be in request edit mode before normal terminal paste will land in a field.
- `Ctrl+V` is also supported as an explicit clipboard paste shortcut.

## Where Requests Are Stored

Saved requests are persisted as a JSON file in the OS-appropriate config directory using the `directories` crate.

Examples:

- macOS: under `~/Library/Application Support/...` or `~/Library/Preferences/...` depending on platform conventions
- Linux: under `~/.config/...`
- Windows: under `%APPDATA%\\...`

The file name is `library.json`.

## Current Limitations

This is intentionally a small first version. It does not currently include:

- folders or collections
- auth helpers
- query param builders
- form-data or multipart support
- request history
- environment variables
- response export or copy-as-curl

## Development Notes

The app is organized into a few small modules:

- `src/app.rs`: app state, key handling, event loop
- `src/ui.rs`: ratatui rendering
- `src/network.rs`: HTTP execution and response formatting
- `src/storage.rs`: persistent request library
- `src/model.rs`: request/response data types and validation
