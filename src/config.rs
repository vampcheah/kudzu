use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

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
    pub openers: HashMap<String, String>,
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
            openers: HashMap::new(),
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
    openers: Option<HashMap<String, String>>,
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

    pub fn opener_for_path(&self, path: &Path) -> &str {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return &self.file_opener;
        };
        self.openers
            .get(&ext.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or(&self.file_opener)
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
    if let Some(v) = file.openers {
        cfg.openers = normalize_openers(v);
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
                "opener" => {
                    let v = value.ok_or_else(|| anyhow::anyhow!("--opener requires =ext:<cmd>"))?;
                    let (ext, cmd) = parse_opener_rule(&v)?;
                    cfg.openers.insert(ext, cmd);
                }
                "file-manager" => {
                    cfg.file_manager =
                        value.ok_or_else(|| anyhow::anyhow!("--file-manager requires =<cmd>"))?;
                }
                "help" | "h" => {
                    print_help();
                    std::process::exit(0);
                }
                "print-config" => {
                    print!("{}", default_config_text(cfg));
                    std::process::exit(0);
                }
                "init-config" => {
                    let path = config_path()
                        .ok_or_else(|| anyhow::anyhow!("could not resolve config path"))?;
                    if path.exists() {
                        bail!("config already exists: {}", path.display());
                    }
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&path, default_config_text(cfg))?;
                    println!("created {}", path.display());
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
             --opener=ext:<cmd>       opener for one file extension (repeatable)\n\
             --file-manager=<cmd>     file manager command (default: xdg-open/open/explorer)\n\
             --osc7                   emit OSC 7 working-directory reports\n\
             --no-osc7                disable OSC 7 reports (default)\n\
             --print-config           print the effective config and exit\n\
             --init-config            create the default config file and exit\n\
             --help                   print this help\n\
         \n\
         CONFIG FILE:\n\
             $XDG_CONFIG_HOME/kudzu/config.toml (or ~/.config/kudzu/config.toml)\n\
         "
    );
}

fn normalize_openers(openers: HashMap<String, String>) -> HashMap<String, String> {
    openers
        .into_iter()
        .map(|(ext, cmd)| (normalize_ext(&ext), cmd))
        .collect()
}

fn normalize_ext(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_ascii_lowercase()
}

fn parse_opener_rule(rule: &str) -> Result<(String, String)> {
    let (ext, cmd) = rule
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("--opener expects ext:<cmd>"))?;
    let ext = normalize_ext(ext);
    if ext.is_empty() {
        bail!("--opener extension cannot be empty");
    }
    if cmd.trim().is_empty() {
        bail!("--opener command cannot be empty");
    }
    Ok((ext, cmd.to_string()))
}

fn default_config_text(cfg: &Config) -> String {
    let mut text = format!(
        "show_hidden = {}\n\
         respect_gitignore = {}\n\
         double_click = \"{}\"\n\
         gui_editor = \"{}\"\n\
         file_opener = \"{}\"\n\
         file_manager = \"{}\"\n\
         osc7 = {}\n\
         \n\
         [openers]\n",
        cfg.show_hidden,
        cfg.respect_gitignore,
        match cfg.double_click {
            DoubleClick::Editor => "editor",
            DoubleClick::Gui => "gui",
        },
        toml_string(&cfg.gui_editor),
        toml_string(&cfg.file_opener),
        toml_string(&cfg.file_manager),
        cfg.osc7
    );
    if cfg.openers.is_empty() {
        text.push_str("# md = \"code -n\"\n# pdf = \"xdg-open\"\n");
    } else {
        let mut keys: Vec<&String> = cfg.openers.keys().collect();
        keys.sort();
        for key in keys {
            let cmd = &cfg.openers[key];
            text.push_str(&format!("{} = \"{}\"\n", toml_key(key), toml_string(cmd)));
        }
    }
    text
}

fn toml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn toml_key(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        s.to_string()
    } else {
        format!("\"{}\"", toml_string(s))
    }
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
            [openers]
            PNG = "imv"
            ".md" = "code"
        "#;
        let mut cfg = Config::default();
        apply_toml(&mut cfg, text).unwrap();
        assert!(cfg.show_hidden);
        assert!(!cfg.respect_gitignore);
        assert_eq!(cfg.double_click, DoubleClick::Gui);
        assert_eq!(cfg.gui_editor, "code -n");
        assert_eq!(cfg.file_opener, "xdg-open");
        assert_eq!(cfg.openers.get("png").map(String::as_str), Some("imv"));
        assert_eq!(cfg.openers.get("md").map(String::as_str), Some("code"));
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
                "--opener=pdf:zathura".to_string(),
                "/tmp".to_string(),
            ],
        )
        .unwrap();
        assert!(cfg.show_hidden);
        assert_eq!(cfg.double_click, DoubleClick::Gui);
        assert_eq!(cfg.file_opener, "wslview");
        assert_eq!(cfg.openers.get("pdf").map(String::as_str), Some("zathura"));
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
