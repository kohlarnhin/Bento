//! Images that AI CLIs (or anything else) write into a project, so Bento can show them next to the
//! terminal: a scan of the project for recent image files, and a reader that hands one image to the
//! webview as a data URL.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "svg", "avif", "bmp"];
/// Dependency, build-cache and tool directories that hold images nobody just made.
const SKIP_DIRS: [&str; 11] = [
    "node_modules", "target", "Pods", "DerivedData", "__pycache__", "venv", "vendor", "bower_components",
    "coverage", "Library", "site-packages",
];
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 40_000;
const MAX_LIMIT: usize = 500;
const MAX_READ_BYTES: u64 = 30 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageArtifact {
    pub path: String,
    pub relative: String,
    pub modified_ms: u64,
    pub size: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    pub path: String,
    pub data_url: String,
    pub size: u64,
    pub modified_ms: u64,
}

fn extension(path: &Path) -> Option<String> {
    path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase)
}

pub fn is_image(path: &Path) -> bool {
    extension(path).is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

fn millis(time: std::io::Result<SystemTime>) -> u64 {
    time.ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_millis() as u64)
}

/// Image files under `root` modified at or after `since_ms`, newest first.
pub fn scan(root: &Path, since_ms: u64, limit: usize) -> Vec<ImageArtifact> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    'walk: while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_ENTRIES {
                break 'walk;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MAX_DEPTH && !skip_dir(&entry.file_name().to_string_lossy()) {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !kind.is_file() || !is_image(&path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let modified_ms = millis(meta.modified());
            if modified_ms < since_ms {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
            found.push(ImageArtifact { path: path.to_string_lossy().into_owned(), relative, modified_ms, size: meta.len() });
        }
    }
    found.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.relative.cmp(&b.relative)));
    found.truncate(limit);
    found
}

/// A path as printed in a terminal: absolute, `~/…`, or relative to the terminal's directory.
pub fn resolve(path: &str, base: Option<&str>) -> PathBuf {
    let expanded = PathBuf::from(crate::expand_tilde(path.trim()));
    if expanded.is_absolute() {
        return expanded;
    }
    match base.map(str::trim).filter(|base| !base.is_empty()) {
        Some(base) => PathBuf::from(crate::expand_tilde(base)).join(expanded),
        None => expanded,
    }
}

fn mime(path: &Path) -> &'static str {
    match extension(path).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        _ => "image/png",
    }
}

pub fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

fn read_image(path: &Path) -> Result<ImageData, String> {
    if !is_image(path) {
        return Err("不是图片文件".into());
    }
    let meta = std::fs::metadata(path).map_err(|_| format!("找不到图片：{}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("找不到图片：{}", path.display()));
    }
    if meta.len() > MAX_READ_BYTES {
        return Err("图片超过 30 MB，无法预览".into());
    }
    let bytes = std::fs::read(path).map_err(|error| format!("读取图片失败：{error}"))?;
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Ok(ImageData {
        data_url: format!("data:{};base64,{}", mime(&path), encode_base64(&bytes)),
        path: path.to_string_lossy().into_owned(),
        size: meta.len(),
        modified_ms: millis(meta.modified()),
    })
}

#[tauri::command]
pub async fn project_images(path: String, since_ms: Option<u64>, limit: Option<usize>) -> Result<Vec<ImageArtifact>, String> {
    let root = PathBuf::from(crate::expand_tilde(&path));
    if !root.is_dir() {
        return Err("项目目录不存在".into());
    }
    let limit = limit.unwrap_or(200).clamp(1, MAX_LIMIT);
    tauri::async_runtime::spawn_blocking(move || scan(&root, since_ms.unwrap_or(0), limit))
        .await
        .map_err(|error| format!("扫描图片失败：{error}"))
}

#[tauri::command]
pub async fn read_image_file(path: String, base: Option<String>) -> Result<ImageData, String> {
    let path = resolve(&path, base.as_deref());
    tauri::async_runtime::spawn_blocking(move || read_image(&path))
        .await
        .map_err(|error| format!("读取图片失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bento-artifacts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn base64_matches_reference_values() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(encode_base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn resolves_relative_paths_against_the_terminal_directory() {
        assert_eq!(resolve("out/a.png", Some("/tmp/project")), PathBuf::from("/tmp/project/out/a.png"));
        assert_eq!(resolve("/abs/a.png", Some("/tmp/project")), PathBuf::from("/abs/a.png"));
        assert_eq!(resolve("a.png", None), PathBuf::from("a.png"));
    }

    #[test]
    fn scan_finds_images_and_skips_dependency_and_hidden_folders() {
        let root = temp_dir("scan");
        for file in ["design/hero.png", "public/logo.svg", "notes.md", "node_modules/pkg/icon.png", ".git/x.png", "deep/a/b/c.JPG"] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
        }
        let mut names: Vec<_> = scan(&root, 0, 50).into_iter().map(|image| image.relative).collect();
        names.sort();
        assert_eq!(names, ["deep/a/b/c.JPG", "design/hero.png", "public/logo.svg"]);
        assert!(scan(&root, u64::MAX, 50).is_empty());
        assert_eq!(scan(&root, 0, 1).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_only_image_files() {
        let root = temp_dir("read");
        std::fs::write(root.join("a.png"), b"foo").unwrap();
        std::fs::write(root.join("a.txt"), b"foo").unwrap();
        let image = read_image(&root.join("a.png")).unwrap();
        assert_eq!(image.data_url, "data:image/png;base64,Zm9v");
        assert!(read_image(&root.join("a.txt")).is_err());
        assert!(read_image(&root.join("missing.png")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
