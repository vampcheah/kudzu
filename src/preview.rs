use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::SystemTime,
};

use crossbeam_channel::Sender;

use crate::{event::AppEvent, filetype};

#[derive(Debug, Clone)]
pub struct Preview {
    pub path: PathBuf,
    pub generation: u64,
    pub lines: Vec<String>,
    pub loading: bool,
}

#[derive(Debug, Default)]
pub struct PreviewState {
    pub current: Option<Preview>,
    generation: u64,
}

impl PreviewState {
    pub fn request(&mut self, path: PathBuf, tx: Sender<AppEvent>) {
        if self
            .current
            .as_ref()
            .map(|p| p.path == path)
            .unwrap_or(false)
        {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.current = Some(Preview {
            path: path.clone(),
            generation,
            lines: vec!["loading...".to_string()],
            loading: true,
        });
        thread::spawn(move || {
            let lines = build_preview(&path, 120);
            let _ = tx.send(AppEvent::PreviewReady {
                generation,
                path,
                lines,
            });
        });
    }

    pub fn accept(&mut self, generation: u64, path: PathBuf, lines: Vec<String>) -> bool {
        let Some(current) = self.current.as_mut() else {
            return false;
        };
        if current.generation != generation || current.path != path {
            return false;
        }
        current.lines = lines;
        current.loading = false;
        true
    }
}

pub fn build_preview(path: &Path, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let name = filetype::short_path(path);
    lines.push(name);
    let Ok(meta) = fs::symlink_metadata(path) else {
        lines.push("unreadable".to_string());
        return lines;
    };
    let kind = filetype::detect_path(path);
    lines.push(format!("{} · {}", kind.label(), human_size(meta.len())));
    if let Ok(modified) = meta.modified()
        && let Ok(age) = modified.elapsed()
    {
        lines.push(format!("modified {} ago", human_duration(age.as_secs())));
    }
    if let Ok(created) = meta.created()
        && let Some(label) = system_time_label(created)
    {
        lines.push(format!("created {label}"));
    }
    lines.push(String::new());

    if meta.is_dir() {
        let mut dirs = 0usize;
        let mut files = 0usize;
        if let Ok(read_dir) = fs::read_dir(path) {
            for entry in read_dir.flatten().take(1000) {
                if entry.path().is_dir() {
                    dirs += 1;
                } else {
                    files += 1;
                }
            }
        }
        lines.push(format!("{dirs} dirs · {files} files"));
        return lines;
    }

    if !meta.is_file() {
        return lines;
    }
    if kind.should_use_file_opener() {
        lines.push(format!("{} content", kind.label()));
        return lines;
    }
    if let Some(summary) = structured_preview(path) {
        lines.extend(summary);
        return lines;
    }
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            lines.push("unreadable".to_string());
            return lines;
        }
    };
    let mut buf = vec![0; 32 * 1024];
    let n = file.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    if filetype::detect_sample(path, &buf).should_use_file_opener() {
        lines.push("binary content".to_string());
        return lines;
    }
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().take(max_lines.saturating_sub(lines.len())) {
        lines.push(line.to_string());
    }
    lines
}

fn structured_preview(path: &Path) -> Option<Vec<String>> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => {
            let text = fs::read_to_string(path).ok()?;
            let trimmed = text.trim_start();
            let kind = if trimmed.starts_with('{') {
                "object"
            } else if trimmed.starts_with('[') {
                "array"
            } else {
                "value"
            };
            Some(vec![format!("json {kind}")])
        }
        "toml" => {
            let text = fs::read_to_string(path).ok()?;
            let value: toml::Value = toml::from_str(&text).ok()?;
            Some(vec![format!("toml {}", value.type_str())])
        }
        _ => None,
    }
}

fn system_time_label(time: SystemTime) -> Option<String> {
    time.elapsed()
        .ok()
        .map(|age| format!("{} ago", human_duration(age.as_secs())))
}

fn human_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{}{}", bytes, UNITS[0])
    } else {
        format!("{:.1}{}", v, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kudzu-preview-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn previews_text_without_binary_dump() {
        let root = tmp("text");
        let file = root.join("note.txt");
        fs::write(&file, "hello\nworld\n").unwrap();
        let lines = build_preview(&file, 20);
        assert!(lines.iter().any(|l| l == "hello"));
    }

    #[test]
    fn previews_binary_as_kind() {
        let root = tmp("bin");
        let file = root.join("image");
        fs::write(&file, b"\x89PNG\r\n\x1a\nmore").unwrap();
        let lines = build_preview(&file, 20);
        assert!(lines.iter().any(|l| l.contains("image content")));
    }
}
