use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Directory,
    Text,
    Image,
    Pdf,
    Archive,
    Audio,
    Video,
    Binary,
    Special,
    Unknown,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Text => "text",
            Self::Image => "image",
            Self::Pdf => "pdf",
            Self::Archive => "archive",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Binary => "binary",
            Self::Special => "special",
            Self::Unknown => "unknown",
        }
    }

    pub fn should_use_file_opener(self) -> bool {
        matches!(
            self,
            Self::Image | Self::Pdf | Self::Archive | Self::Audio | Self::Video | Self::Binary
        )
    }
}

pub fn detect_path(path: &Path) -> FileKind {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return FileKind::Unknown;
    };
    if meta.is_dir() {
        return FileKind::Directory;
    }
    if !meta.is_file() && !meta.file_type().is_symlink() {
        return FileKind::Special;
    }
    let mut sample = [0; 8192];
    let n = fs::File::open(path)
        .and_then(|mut file| file.read(&mut sample))
        .unwrap_or(0);
    detect_sample(path, &sample[..n])
}

pub fn detect_sample(path: &Path, sample: &[u8]) -> FileKind {
    if sample.starts_with(b"\x89PNG\r\n\x1a\n")
        || sample.starts_with(&[0xff, 0xd8, 0xff])
        || sample.starts_with(b"GIF87a")
        || sample.starts_with(b"GIF89a")
        || sample.starts_with(b"RIFF") && sample.get(8..12) == Some(b"WEBP")
        || sample.starts_with(b"BM")
    {
        return FileKind::Image;
    }
    if sample.starts_with(b"%PDF-") {
        return FileKind::Pdf;
    }
    if sample.starts_with(b"PK\x03\x04")
        || sample.starts_with(b"\x1f\x8b")
        || sample.starts_with(b"Rar!\x1a\x07")
        || sample.starts_with(b"7z\xbc\xaf\x27\x1c")
    {
        return FileKind::Archive;
    }
    if sample.starts_with(b"ID3")
        || sample.starts_with(b"OggS")
        || sample.starts_with(b"fLaC")
        || sample.starts_with(b"RIFF") && sample.get(8..12) == Some(b"WAVE")
    {
        return FileKind::Audio;
    }
    if sample.len() > 12 && sample.get(4..8) == Some(b"ftyp") {
        return FileKind::Video;
    }
    if sample.contains(&0) || std::str::from_utf8(sample).is_err() {
        return FileKind::Binary;
    }
    if extension_is_image(path) {
        return FileKind::Image;
    }
    if extension_is_media(path, &["mp3", "wav", "flac", "ogg", "m4a"]) {
        return FileKind::Audio;
    }
    if extension_is_media(path, &["mp4", "mkv", "mov", "webm", "avi"]) {
        return FileKind::Video;
    }
    FileKind::Text
}

pub fn extension_is_image(path: &Path) -> bool {
    extension_is_media(
        path,
        &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "avif",
            "heic", "heif",
        ],
    )
}

fn extension_is_media(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map(|e| exts.contains(&e.as_str()))
        .unwrap_or(false)
}

pub fn short_path(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn parent_dirs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_magic_bytes_before_extension() {
        assert_eq!(
            detect_sample(Path::new("noext"), b"\x89PNG\r\n\x1a\n"),
            FileKind::Image
        );
        assert_eq!(detect_sample(Path::new("doc"), b"%PDF-1.7"), FileKind::Pdf);
        assert_eq!(
            detect_sample(Path::new("zip"), b"PK\x03\x04"),
            FileKind::Archive
        );
    }

    #[test]
    fn detects_text_and_binary_samples() {
        assert_eq!(detect_sample(Path::new("a.txt"), b"hello"), FileKind::Text);
        assert_eq!(
            detect_sample(Path::new("a.bin"), b"\0hello"),
            FileKind::Binary
        );
    }
}
