//! A workspace's files, to read and nothing else, for seeing what the app is really working from.

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::app::AppError;

/// More than this isn't worth sending to a browser to read by eye.
const MOST: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct FileInfo {
    /// Relative to the workspace, with `/` between folders.
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct FileText {
    pub path: String,
    pub size: u64,
    /// `None` if it isn't text.
    pub text: Option<String>,
    pub truncated: bool,
}

/// Everything there is to read, by path. Git's own files, the lock and half-written files aren't the workspace's data,
/// and a symlink could lead anywhere.
pub fn list(dir: &Path) -> Result<Vec<FileInfo>> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<FileInfo>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.is_dir() && name != ".git" {
                walk(&entry.path(), &format!("{prefix}{name}/"), out)?;
            } else if meta.is_file() && name != crate::app::LOCK && !(name.starts_with('.') && name.ends_with(".tmp")) {
                out.push(FileInfo { path: format!("{prefix}{name}"), size: meta.len() });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// One of the files `list` gives. Asking by anything else finds nothing, so a path can't climb out
/// of the workspace.
pub fn read(dir: &Path, path: &str) -> Result<FileText, AppError> {
    let Some(info) = list(dir)?.into_iter().find(|f| f.path == path) else {
        return Err(AppError::NotFound(format!("file {path}")));
    };
    let mut bytes = Vec::new();
    let file = std::fs::File::open(dir.join(path)).map_err(anyhow::Error::from)?;
    file.take(MOST).read_to_end(&mut bytes).map_err(anyhow::Error::from)?;
    let truncated = info.size > MOST;
    let text = text_of(bytes, truncated);
    Ok(FileText { path: info.path, size: info.size, text, truncated })
}

/// Cutting a file short can land inside a character, and that alone doesn't make it not text.
fn text_of(mut bytes: Vec<u8>, truncated: bool) -> Option<String> {
    if truncated {
        let good = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => return None,
        };
        bytes.truncate(good);
    }
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_cut_in_half_by_the_limit_is_still_text() {
        let cut = "a€".as_bytes()[..2].to_vec();
        assert_eq!(text_of(cut.clone(), true).as_deref(), Some("a"));
        assert_eq!(text_of(cut, false), None, "the whole file, so it really is broken");
    }

    #[test]
    fn binary_isnt_text() {
        assert_eq!(text_of(vec![b'a', 0, b'b'], false), None);
        assert_eq!(text_of(vec![0xff, 0xfe, b'a'], true), None);
    }
}
