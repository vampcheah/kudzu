use std::{env, fs, path::PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer};

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
    pub file_opener: String,
    pub file_manager: String,
    pub osc7: bool,
    pub root: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            show_hidden: false,
            respect_gitignore: true,
            double_click: DoubleClick::Editor,
            gui_editor: default_file_opener().to_string(),
            file_opener: default_file_opener().to_string(),
            file_manager: default_file_manager().to_string(),
            osc7: false,
            root: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    show_hidden: Option<bool>,
    respect_gitignore: Option<bool>,
    double_click: Option<DoubleClick>,
    gui_editor: Option<String>,
    file_opener: Option<String>,
    file_manager: Option<String>,
    osc7: Option<bool>,
}

impl<'de> Deserialize<'de> for DoubleClick {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_double_click(&value).map_err(serde::de::Error::custom)
    }
}

fn default_file_manager() -> &'static str {
    default_file_opener()
}

fn default_file_opener() -> &'static str {
    std::cfg_select! {
        target_os = "macos" => "open",
        target_os = "windows" => "explorer",
        _ => "xdg-open",
    }
}

impl Config {
    /// Load from `~/.config/kudzu/config.toml` (missing file is fine) then
    /// overlay CLI args from `env::args()`.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::default();
        if let Some(path) = config_path()
            && path.exists()
        {
            let text = fs::read_to_string(&path)?;
            apply_toml(&mut cfg, &text)?;
        }
        apply_cli(&mut cfg, env::args().skip(1))?;
        Ok(cfg)
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(dir) = env::var("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("kudzu/config.toml"));
    }
    env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".config/kudzu/config.toml"))
}

/// Parse the user config as real TOML, while keeping the accepted surface
/// intentionally small and explicit.
fn apply_toml(cfg: &mut Config, text: &str) -> Result<()> {
    let file: FileConfig = toml::from_str(text).map_err(|e| anyhow::anyhow!("config: {}", e))?;
    if let Some(v) = file.show_hidden {
        cfg.show_hidden = v;
    }
    if let Some(v) = file.respect_gitignore {
        cfg.respect_gitignore = v;
    }
    if let Some(v) = file.double_click {
        cfg.double_click = v;
    }
    if let Some(v) = file.gui_editor {
        cfg.gui_editor = v;
    }
    if let Some(v) = file.file_opener {
        cfg.file_opener = v;
    }
    if let Some(v) = file.file_manager {
        cfg.file_manager = v;
    }
    if let Some(v) = file.osc7 {
        cfg.osc7 = v;
    }
    Ok(())
}

fn parse_double_click(s: &str) -> Result<DoubleClick, String> {
    match s {
        "editor" | "shell" => Ok(DoubleClick::Editor),
        "gui" => Ok(DoubleClick::Gui),
        other => Err(format!("expected `editor`/`gui`, got `{}`", other)),
    }
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
                "osc7" => cfg.osc7 = true,
                "no-osc7" => cfg.osc7 = false,
                "double-click" => {
                    let v = value
                        .ok_or_else(|| anyhow::anyhow!("--double-click requires =editor|gui"))?;
                    cfg.double_click = parse_double_click(&v)
                        .map_err(|e| anyhow::anyhow!("--double-click: {}", e))?;
                }
                "gui-editor" => {
                    cfg.gui_editor =
                        value.ok_or_else(|| anyhow::anyhow!("--gui-editor requires =<cmd>"))?;
                }
                "file-opener" => {
                    cfg.file_opener =
                        value.ok_or_else(|| anyhow::anyhow!("--file-opener requires =<cmd>"))?;
                }
                "file-manager" => {
                    cfg.file_manager =
                        value.ok_or_else(|| anyhow::anyhow!("--file-manager requires =<cmd>"))?;
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
             --gui-editor=<cmd>       GUI editor command (default: system opener)\n\
             --file-opener=<cmd>      system opener for images/binary files\n\
             --file-manager=<cmd>     file manager command (default: xdg-open/open/explorer)\n\
             --osc7                   emit OSC 7 working-directory reports\n\
             --no-osc7                disable OSC 7 reports (default)\n\
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
            file_opener = "xdg-open"
        "#;
        let mut cfg = Config::default();
        apply_toml(&mut cfg, text).unwrap();
        assert!(cfg.show_hidden);
        assert!(!cfg.respect_gitignore);
        assert_eq!(cfg.double_click, DoubleClick::Gui);
        assert_eq!(cfg.gui_editor, "code -n");
        assert_eq!(cfg.file_opener, "xdg-open");
    }

    #[test]
    fn cli_overrides_config() {
        let mut cfg = Config {
            show_hidden: false,
            ..Config::default()
        };
        apply_cli(
            &mut cfg,
            vec![
                "--show-hidden".to_string(),
                "--double-click=gui".to_string(),
                "--file-opener=wslview".to_string(),
                "/tmp".to_string(),
            ],
        )
        .unwrap();
        assert!(cfg.show_hidden);
        assert_eq!(cfg.double_click, DoubleClick::Gui);
        assert_eq!(cfg.file_opener, "wslview");
        assert_eq!(cfg.root, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn unknown_flag_errors() {
        let mut cfg = Config::default();
        assert!(apply_cli(&mut cfg, vec!["--wat".to_string()]).is_err());
    }

    #[test]
    fn unknown_config_key_errors() {
        let mut cfg = Config::default();
        assert!(apply_toml(&mut cfg, "wat = true").is_err());
    }
}
