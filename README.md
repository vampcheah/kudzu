# kudzu

A lightweight terminal file browser (TUI) written in Rust. Keyboard-driven, fuzzy search, respects `.gitignore`, auto-refreshes on filesystem changes.

## Installation

**Linux / macOS**

```bash
git clone https://github.com/vampcheah/kudzu.git
cd kudzu
sudo make install            # installs to /usr/local/bin, creates kz symlink
# Without sudo:
PREFIX=$HOME/.local make install
```

If `cargo` is not found, `make install` installs the Rust toolchain via `rustup` automatically.

**Windows (PowerShell)**

```powershell
git clone https://github.com/vampcheah/kudzu.git
cd kudzu
.\install.ps1
```

**Via cargo**

```bash
cargo install --path .
```

Note: `cargo install` does not create the `kz` alias. Add one manually if wanted:

```bash
ln -sf ~/.cargo/bin/kudzu ~/.cargo/bin/kz
```

## Usage

```bash
kudzu              # open the current directory
kudzu ~/projects   # specify a root directory
```

Editor is resolved as `$EDITOR` → `$VISUAL` → `vi`.

## Key bindings

### Normal mode

| Key | Action |
| --- | --- |
| `s` / `↓` | Move down |
| `w` / `↑` | Move up |
| `u` / `←` | Collapse dir / jump to parent / ascend root |
| `l` / `→` / `Space` | Expand directory |
| `f` | Focus into selected directory |
| `Enter` | Expand dir or open file (per `double_click` config) |
| `o` | Open file in `$EDITOR` |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `Ctrl-d` / `PageDown` | Scroll down 10 |
| `Ctrl-u` / `PageUp` | Scroll up 10 |
| `/` | Enter search mode |
| `n` / `N` | New file / new folder |
| `R` | Rename |
| `D` | Move to trash (confirm with `y`) |
| `M` | Open in file manager |
| `.` | Toggle hidden files |
| `i` | Toggle `.gitignore` handling |
| `r` | Rescan |
| `h` | Toggle help popup |
| `q` / `Ctrl-c` | Quit |

### Search mode

| Key | Action |
| --- | --- |
| (type) | Filter live |
| `↑` / `↓` | Select match |
| `Enter` | Jump to match / open file |
| `Backspace` / `Ctrl-w` | Delete char / word |
| `Esc` / `Ctrl-c` | Exit search |

### Mouse

- Click to select, double-click to expand dir or open file
- Right-click for context menu (new file/folder, rename, delete, open)
- Scroll to navigate

## Configuration

`~/.config/kudzu/config.toml` (or `$XDG_CONFIG_HOME/kudzu/config.toml`) is parsed as TOML:

```toml
show_hidden = false          # show hidden files at startup
respect_gitignore = true     # respect .gitignore
double_click = "editor"      # "editor" (terminal $EDITOR) or "gui" (GUI app)
gui_editor = "xdg-open"      # command for double_click = "gui", e.g. "code -n"
file_manager = "xdg-open"    # command for M key; defaults to open/explorer on macOS/Windows
osc7 = false                 # emit OSC 7 working-directory escape sequences
```

`gui_editor` and `file_manager` support simple quoting, e.g. `"/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code" -n`.

Command-line flags override config:

```bash
kudzu --show-hidden --double-click=gui --gui-editor=code
kudzu --no-ignore ~/projects
kudzu --osc7                 # enable OSC 7 reports (e.g. for terminal tab titles)
kudzu --help
```

## License

MIT
