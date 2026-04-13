# kudzu

A lightweight terminal file browser (TUI) written in Rust, built on [ratatui](https://github.com/ratatui/ratatui).

Keyboard-driven, fuzzy search, respects `.gitignore`, auto-refreshes on filesystem changes — ideal for quickly locating and opening project files from the terminal.

## Features

- 🌲 Expandable/collapsible tree view
- 🔍 Fuzzy search (powered by [nucleo-matcher](https://github.com/helix-editor/nucleo)), press Enter to jump
- 🙈 Respects `.gitignore`, toggle hidden files on demand
- 👀 Filesystem watcher (powered by [notify](https://github.com/notify-rs/notify)), auto-refresh on changes
- 🖱️ Mouse support: scroll, click to select, double-click to expand or open, right-click for a context menu
- ✏️ `Enter` / `o` opens the selected file in `$EDITOR`
- ⚡ Fast startup — only expanded directories are scanned

## Installation

### Linux / macOS (one-shot)

```bash
git clone https://github.com/<your-user>/kudzu.git
cd kudzu
sudo make install            # installs to /usr/local/bin by default
# Or install to your home directory without sudo:
PREFIX=$HOME/.local make install
```

If `cargo` is not found, `make install` installs the stable toolchain via `rustup` automatically (requires `curl`). To uninstall:

```bash
sudo make uninstall          # or PREFIX=$HOME/.local make uninstall
```

### Windows (PowerShell one-shot)

```powershell
git clone https://github.com/<your-user>/kudzu.git
cd kudzu
.\install.ps1                # installs to %LOCALAPPDATA%\Programs\kudzu and adds it to user PATH
.\install.ps1 -Uninstall     # uninstall
```

If `cargo` is not found, the script downloads `rustup-init.exe` and installs the stable toolchain. If script execution is blocked on first run: `Set-ExecutionPolicy -Scope Process Bypass`.

### Via cargo

Requires Rust 1.70+.

```bash
cargo install --path .
# Or just build:
cargo build --release && ./target/release/kudzu
```

## Usage

```bash
kudzu              # open the current directory
kudzu ~/projects   # specify a root directory
```

The editor is picked in the order `$EDITOR` > `$VISUAL` > `vi`.

## Key bindings

### Normal mode

| Key | Action |
| --- | --- |
| `s` / `↓` | Move down |
| `w` / `↑` | Move up |
| `u` / `←` | Collapse directory / jump to parent / at root, ascend one level |
| `l` / `→` / `Space` | Expand directory |
| `f` | Focus into selected directory (make it the new root) |
| `Enter` | Expand directory or open file in editor |
| `o` | Open file in editor |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `Ctrl-d` / `PageDown` | Scroll down 10 lines |
| `Ctrl-u` / `PageUp` | Scroll up 10 lines |
| `/` | Enter search mode |
| `n` | New file in the selected directory |
| `N` | New folder in the selected directory |
| `R` | Rename the selected file/folder |
| `D` | Delete the selected file/folder (confirm with `y`) |
| `M` | Open the selected directory in the system file manager |
| `.` | Toggle hidden files |
| `i` | Toggle `.gitignore` handling |
| `r` | Rescan |
| `h` | Toggle help popup |
| `q` / `Ctrl-c` | Quit |

### Search mode

| Key | Action |
| --- | --- |
| (type characters) | Filter live |
| `↑` / `↓` | Select match |
| `Enter` | Jump to match (opens in editor if it's a file) |
| `Backspace` | Delete one character |
| `Ctrl-w` | Delete one word |
| `Esc` / `Ctrl-c` | Exit search |

### Context menu (right-click)

Right-click inside the tree to open a menu. The items shown depend on what
was clicked:

| Target | Items |
| --- | --- |
| File | New Folder, New File, Rename, Open File |
| Folder (non-root) | New Folder, New File, Rename, Open Folder |
| Root folder / empty area | New Folder, New File, Open Folder |

`New File` / `New Folder` create the entry inside the clicked folder (or the
parent folder when a file was clicked). `Open File` opens the file in
`$EDITOR`; `Open Folder` reveals the folder in the system file manager.

Navigate with `↑` / `↓` (or `j` / `k` / `w` / `s`), activate with `Enter` or
a left click, dismiss with `Esc` or a click outside the popup.

### Input prompt (new file / new folder / rename)

| Key | Action |
| --- | --- |
| (type characters) | Insert at the cursor |
| `←` / `Ctrl-b` | Move cursor left |
| `→` / `Ctrl-f` | Move cursor right |
| `Home` / `Ctrl-a` | Jump to start |
| `End` / `Ctrl-e` | Jump to end |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character at cursor |
| `Ctrl-w` | Delete word before cursor |
| `Ctrl-u` | Delete from start to cursor |
| `Enter` | Confirm |
| `Esc` / `Ctrl-c` | Cancel |

## Configuration

Config file location: `$XDG_CONFIG_HOME/kudzu/config.toml` (usually `~/.config/kudzu/config.toml`). All fields are optional; defaults are used when absent.

```toml
show_hidden = false          # show hidden files/folders at startup
respect_gitignore = true     # respect .gitignore
double_click = "editor"      # double-click behavior: "editor" (terminal $EDITOR) or "gui" (GUI editor)
gui_editor = "xdg-open"      # command used when double_click = "gui"; supports args like "code -n"
file_manager = "xdg-open"    # command used by `M` to open a folder; defaults to `open` on macOS, `explorer` on Windows
```

Command-line flags override the config file:

```bash
kudzu --show-hidden --double-click=gui --gui-editor=code
kudzu --file-manager=nautilus
kudzu --no-ignore ~/projects
kudzu --help
```

Double-click behavior:
- `editor`: suspends the TUI, opens the file in `$EDITOR`/`$VISUAL`/`vi` (e.g. vim, nano); returns to the TUI on exit.
- `gui`: spawns the GUI editor in the background without suspending the TUI (e.g. VS Code, Sublime, `xdg-open`).

## License

MIT
