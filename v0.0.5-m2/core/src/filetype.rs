//! Dosya türü sınıflandırması (ikon, arama çipleri ve menüler için).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileClass {
    Folder,
    Archive,
    Apk,
    Image,
    Video,
    Audio,
    Document,
    Text,
    Other,
}

const ARCHIVE: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz2", "tbz", "xz", "txz", "lz4", "zst", "jar", "lzma", "cab", "iso",
];
const APK: &[&str] = &["apk", "xapk", "apks", "apkm", "aab"];
const IMAGE: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "heif", "svg", "ico", "tif", "tiff"];
const VIDEO: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "3gp", "flv", "wmv", "m4v", "ts"];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "wma", "amr", "mid"];
const DOCUMENT: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "epub", "csv"];
const TEXT: &[&str] = &["txt", "md", "log", "json", "xml", "html", "htm", "css", "js", "rs", "toml", "yml", "yaml", "ini", "cfg", "conf", "sh", "py", "java", "kt", "c", "h", "cpp"];

/// Uzantı (küçük harf). "a.tar.gz" için "gz" döner.
pub fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None; // ".gizli" uzantı sayılmaz
    }
    Some(ext.to_lowercase())
}

pub fn classify(name: &str) -> FileClass {
    let Some(ext) = extension(name) else { return FileClass::Other };
    let e = ext.as_str();
    if APK.contains(&e) {
        FileClass::Apk
    } else if ARCHIVE.contains(&e) {
        FileClass::Archive
    } else if IMAGE.contains(&e) {
        FileClass::Image
    } else if VIDEO.contains(&e) {
        FileClass::Video
    } else if AUDIO.contains(&e) {
        FileClass::Audio
    } else if DOCUMENT.contains(&e) {
        FileClass::Document
    } else if TEXT.contains(&e) {
        FileClass::Text
    } else {
        FileClass::Other
    }
}

/// İkon rozeti için kısa etiket: "ZIP", "APK", "PNG"...
pub fn badge_label(name: &str) -> String {
    match extension(name) {
        Some(e) => e.chars().take(4).collect::<String>().to_uppercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_basic() {
        assert_eq!(classify("a.ZIP"), FileClass::Archive);
        assert_eq!(classify("x.apk"), FileClass::Apk);
        assert_eq!(classify("p.JPG"), FileClass::Image);
        assert_eq!(classify("m.mkv"), FileClass::Video);
        assert_eq!(classify("s.mp3"), FileClass::Audio);
        assert_eq!(classify("d.pdf"), FileClass::Document);
        assert_eq!(classify("n.txt"), FileClass::Text);
        assert_eq!(classify("noext"), FileClass::Other);
        assert_eq!(classify(".dlprovider"), FileClass::Other);
        assert_eq!(classify("a.tar.gz"), FileClass::Archive);
    }

    #[test]
    fn labels() {
        assert_eq!(badge_label("a.tar.gz"), "GZ");
        assert_eq!(badge_label("movie.webm"), "WEBM");
        assert_eq!(badge_label("README"), "");
    }
}
