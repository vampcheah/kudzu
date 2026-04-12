use std::{env, fs, path::PathBuf};

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleClick {
    Editor,
    Gui,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub show_hidden: bool,
    pub respect_gitignore: bool,
    pub double_click: DoubleClick,
    pub gui_editor: String,
    pub root: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            show_hidden: false,
            respect_gitignore: true,
            double_click: DoubleClick::Editor,
            gui_editor: "xdg-open".to_string(),
            root: None,
        }
    }
}

impl Config {
    /// Load from `~/.config/kudzu/config.toml` (missing file is fine) then
    /// overlay CLI args from `env::args()`.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::default();
        if let Some(path) = config_path() {
            if path.exists() {
                let text = fs::read_to_string(&path)?;
                apply_toml(&mut cfg, &text)?;
            }
        }
        apply_cli(&mut cfg, env::args().skip(1))?;
        Ok(cfg)
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(dir) = env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("kudzu/config.toml"));
        }
    }
    env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".config/kudzu/config.toml"))
}

/// Tiny subset of TOML: `key = value` lines. Values may be `true`/`false`,
/// bare identifiers, or `"quoted strings"`. Lines beginning with `#` or
/// blank are ignored. Sections (`[...]`) are skipped silently.
fn apply_toml(cfg: &mut Config, text: &str) -> Result<()> {
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some(kv) => kv,
            None => bail!("config: line {}: expected `key = value`", lineno + 1),
        };
        let key = key.trim();
        let value = strip_comment(value.trim());
        let value = unquote(value);
        set_key(cfg, key, &value).map_err(|e| anyhow::anyhow!("config: {}: {}", key, e))?;
    }
    Ok(())
}

fn strip_comment(s: &str) -> &str {
    // Only strip `#` when outside of a quoted string.
    let mut in_str = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return s[..i].trim_end(),
            _ => {}
        }
    }
    s
}

fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn parse_bool(s: &str) -> Result<bool, String> {
    match s {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("expected true/false, got `{}`", other)),
    }
}

fn parse_double_click(s: &str) -> Result<DoubleClick, String> {
    match s {
        "editor" | "shell" => Ok(DoubleClick::Editor),
        "gui" => Ok(DoubleClick::Gui),
        other => Err(format!("expected `editor`/`gui`, got `{}`", other)),
    }
}

fn set_key(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    match key {
        "show_hidden" => cfg.show_hidden = parse_bool(value)?,
        "respect_gitignore" => cfg.respect_gitignore = parse_bool(value)?,
        "double_click" => cfg.double_click = parse_double_click(value)?,
        "gui_editor" => cfg.gui_editor = value.to_string(),
        other => return Err(format!("unknown key `{}`", other)),
    }
    Ok(())
}

fn apply_cli<I: IntoIterator<Item = String>>(cfg: &mut Config, args: I) -> Result<()> {
    let mut positional: Option<PathBuf> = None;
    for arg in args {
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, value) = match rest.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (rest, None),
            };
            match name {
                "show-hidden" => cfg.show_hidden = true,
                "hide-hidden" | "no-show-hidden" => cfg.show_hidden = false,
                "ignore" | "respect-gitignore" => cfg.respect_gitignore = true,
                "no-ignore" => cfg.respect_gitignore = false,
                "double-click" => {
                    let v = value.ok_or_else(|| {
                        anyhow::anyhow!("--double-click requires =editor|gui")
                    })?;
                    cfg.double_click = parse_double_click(&v)
                        .map_err(|e| anyhow::anyhow!("--double-click: {}", e))?;
                }
                "gui-editor" => {
                    cfg.gui_editor = value
                        .ok_or_else(|| anyhow::anyhow!("--gui-editor requires =<cmd>"))?;
                }
                "help" | "h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => bail!("unknown flag --{}", other),
            }
        } else if positional.is_none() {
            positional = Some(PathBuf::from(arg));
        } else {
            bail!("unexpected positional argument: {}", arg);
        }
    }
    if positional.is_some() {
        cfg.root = positional;
    }
    Ok(())
}

fn print_help() {
    println!(
        "kudzu — terminal file browser\n\
         \n\
         USAGE:\n\
             kudzu [OPTIONS] [PATH]\n\
         \n\
         OPTIONS:\n\
             --show-hidden            show dotfiles by default\n\
             --hide-hidden            hide dotfiles by default\n\
             --no-ignore              don't respect .gitignore\n\
             --ignore                 respect .gitignore (default)\n\
             --double-click=editor    double-click opens $EDITOR (default)\n\
             --double-click=gui       double-click spawns GUI editor\n\
             --gui-editor=<cmd>       GUI editor command (default: xdg-open)\n\
             --help                   print this help\n\
         \n\
         CONFIG FILE:\n\
             $XDG_CONFIG_HOME/kudzu/config.toml (or ~/.config/kudzu/config.toml)\n\
         "
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toml_basics() {
        let text = r#"
            # comment
            show_hidden = true
            respect_gitignore = false
            double_click = "gui"
            gui_editor = "code -n"   # trailing comment
        "#;
        let mut cfg = Config::default();
        apply_toml(&mut cfg, text).unwrap();
        assert!(cfg.show_hidden);
        assert!(!cfg.respect_gitignore);
        assert_eq!(cfg.double_click, DoubleClick::Gui);
        assert_eq!(cfg.gui_editor, "code -n");
    }

    #[test]
    fn cli_overrides_config() {
        let mut cfg = Config::default();
        cfg.show_hidden = false;
        apply_cli(
            &mut cfg,
            vec![
                "--show-hidden".to_string(),
                "--double-click=gui".to_string(),
                "/tmp".to_string(),
            ],
        )
        .unwrap();
        assert!(cfg.show_hidden);
        assert_eq!(cfg.double_click, DoubleClick::Gui);
        assert_eq!(cfg.root, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn unknown_flag_errors() {
        let mut cfg = Config::default();
        assert!(apply_cli(&mut cfg, vec!["--wat".to_string()]).is_err());
    }
}
