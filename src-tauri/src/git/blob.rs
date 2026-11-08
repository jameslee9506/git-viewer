//! Reading file contents at a revision, from the index, or from disk.

use crate::error::{AppError, AppResult};
use crate::git::cmd::Git;
use crate::git::commit::{relative_path, validate_oid, validate_path, validate_rev, worktree_dir};
use crate::model::{BlobContent, BlobEncoding, BlobSpec};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::path::Path;

/// Default cap when `max_bytes` is None.
pub const DEFAULT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Bytes inspected for NUL to classify content as binary (git's heuristic).
const BINARY_SNIFF: usize = 8000;

/// Contents for `spec`. `max_bytes` None = 16 MiB, 0 = unlimited; larger contents are reported with their
/// size and `truncated: true` but without content (`binary` is only determined when content is read).
/// Text encoding returns `text` (lossy UTF-8) unless the content is binary, which then has neither `text`
/// nor `base64`; Base64 encoding always returns `base64`.
/// Revision, index and oid specs read the blob with `git cat-file` (a revision may be a stage like `:2`;
/// an unmerged index path reads stage 2); worktree specs read the file from disk (a symlink yields its
/// target path, which is what git stores for it).
pub fn read_blob(
    repo: &Path,
    spec: &BlobSpec,
    encoding: BlobEncoding,
    max_bytes: Option<u64>,
) -> AppResult<BlobContent> {
    let max = match max_bytes {
        None => DEFAULT_MAX_BYTES,
        Some(0) => u64::MAX,
        Some(n) => n,
    };
    let (data, path) = match spec {
        BlobSpec::Rev { rev, path } => {
            validate_rev(rev)?;
            validate_path(path)?;
            // A stage (`:2` → `:2:<path>`, "ours" of a conflict) is the only revision allowed to contain `:`.
            if colon_outside_braces(rev) && !is_stage(rev) {
                return Err(AppError::msg(format!("invalid revision: {rev}")));
            }
            (read_object(repo, &format!("{rev}:{path}"), max)?, Some(path))
        }
        BlobSpec::Index { cwd, path } => {
            validate_path(path)?;
            let cwd = worktree_dir(cwd)?;
            // `:0:` pins stage 0 so names like `1:x` or `/x` cannot be read as `:<stage>:` or `:/<regex>`.
            // An unmerged path has no stage-0 entry: its index side is then "ours" (stage 2).
            let data = read_object(cwd, &format!(":0:{path}"), max)
                .or_else(|e| read_object(cwd, &format!(":2:{path}"), max).map_err(|_| e))?;
            (data, Some(path))
        }
        BlobSpec::Oid { oid } => {
            validate_oid(oid)?;
            (read_object(repo, oid, max)?, None)
        }
        BlobSpec::Worktree { cwd, path } => (read_file(worktree_dir(cwd)?, path, max)?, Some(path)),
    };
    Ok(content(data, encoding, path.and_then(|p| mime_for(p))))
}

/// Size plus the bytes when they fit under the cap.
struct Data {
    size: u64,
    bytes: Option<Vec<u8>>,
}

fn read_object(cwd: &Path, object: &str, max: u64) -> AppResult<Data> {
    let size = Git::new(cwd).args(["cat-file", "-s", object]).run_string()?;
    let size: u64 =
        size.trim().parse().map_err(|_| AppError::msg(format!("unexpected size of {object}: {}", size.trim())))?;
    if size > max {
        return Ok(Data { size, bytes: None });
    }
    let bytes = Git::new(cwd).args(["cat-file", "blob", object]).run()?;
    Ok(Data { size: bytes.len() as u64, bytes: Some(bytes) })
}

fn read_file(cwd: &Path, path: &str, max: u64) -> AppResult<Data> {
    let full = cwd.join(relative_path(path)?);
    let meta = std::fs::symlink_metadata(&full).map_err(|e| file_error(path, e))?;
    if meta.file_type().is_symlink() {
        let target = os_bytes(std::fs::read_link(&full).map_err(|e| file_error(path, e))?.into_os_string());
        let size = target.len() as u64;
        return Ok(Data { size, bytes: (size <= max).then_some(target) });
    }
    if meta.is_dir() {
        return Err(AppError::msg(format!("{path} is a directory")));
    }
    if !meta.is_file() {
        return Err(AppError::msg(format!("{path} is not a regular file")));
    }
    if meta.len() > max {
        return Ok(Data { size: meta.len(), bytes: None });
    }
    let bytes = std::fs::read(&full).map_err(|e| file_error(path, e))?;
    Ok(Data { size: bytes.len() as u64, bytes: Some(bytes) })
}

fn file_error(path: &str, e: std::io::Error) -> AppError {
    if e.kind() == std::io::ErrorKind::NotFound {
        AppError::msg(format!("{path} does not exist"))
    } else {
        AppError::Io(e)
    }
}

fn os_bytes(s: std::ffi::OsString) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        s.into_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

fn content(data: Data, encoding: BlobEncoding, mime: Option<String>) -> BlobContent {
    let Some(bytes) = data.bytes else {
        return BlobContent { size: data.size, truncated: true, mime, ..Default::default() };
    };
    let binary = memchr::memchr(0, &bytes[..bytes.len().min(BINARY_SNIFF)]).is_some();
    let (text, base64) = match encoding {
        BlobEncoding::Base64 => (None, Some(STANDARD.encode(&bytes))),
        // Binary content read as text is only flagged: encoding it would ship up to 4/3 × max_bytes of JSON
        // that the viewer discards (it asks for Base64 when it renders bytes, e.g. images).
        BlobEncoding::Text if binary => (None, None),
        BlobEncoding::Text => {
            let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
            (Some(text), None)
        }
    };
    BlobContent { size: data.size, binary, truncated: false, text, base64, mime }
}

/// Image MIME type from the file extension.
fn mime_for(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    Some(mime.to_string())
}

/// An index stage `:0` … `:3` (with `:<path>` appended it names that stage's blob).
fn is_stage(rev: &str) -> bool {
    matches!(rev.as_bytes(), [b':', b'0'..=b'3'])
}

/// git ends the revision part of `<rev>:<path>` at the first `:` outside `{…}` (`HEAD@{1}` is fine).
fn colon_outside_braces(rev: &str) -> bool {
    let mut depth = 0u32;
    for b in rev.bytes() {
        match b {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(bytes: &[u8]) -> Data {
        Data { size: bytes.len() as u64, bytes: Some(bytes.to_vec()) }
    }

    #[test]
    fn text_binary_and_truncated() {
        let c = content(data("한글\r\nline".as_bytes()), BlobEncoding::Text, None);
        assert_eq!((c.text.as_deref(), c.base64.is_none(), c.binary, c.size), (Some("한글\r\nline"), true, false, 12));
        let c = content(data(b"\xffab"), BlobEncoding::Text, None);
        assert_eq!(c.text.as_deref(), Some("\u{fffd}ab"));
        let c = content(data(b"PNG\0\x01"), BlobEncoding::Text, mime_for("img/a.PNG"));
        assert_eq!((c.binary, c.text, c.base64, c.size), (true, None, None, 5));
        let c = content(data(b"PNG\0\x01"), BlobEncoding::Base64, mime_for("img/a.PNG"));
        assert_eq!((c.binary, c.text.is_none(), c.base64.as_deref()), (true, true, Some("UE5HAAE=")));
        assert_eq!(c.mime.as_deref(), Some("image/png"));
        let c = content(data(b"<svg/>"), BlobEncoding::Base64, mime_for("icon.svg"));
        assert_eq!(
            (c.binary, c.text, c.base64.as_deref(), c.mime.as_deref()),
            (false, None, Some("PHN2Zy8+"), Some("image/svg+xml"))
        );
        let mut late_nul = vec![b'a'; BINARY_SNIFF];
        late_nul.push(0);
        assert!(!content(data(&late_nul), BlobEncoding::Text, None).binary);
        let c = content(Data { size: 99, bytes: None }, BlobEncoding::Text, None);
        assert_eq!((c.size, c.truncated, c.binary, c.text, c.base64), (99, true, false, None, None));
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("a/b/photo.JPEG").as_deref(), Some("image/jpeg"));
        assert_eq!(mime_for("x.tif").as_deref(), Some("image/tiff"));
        assert_eq!(mime_for("favicon.ico").as_deref(), Some("image/x-icon"));
        assert_eq!(mime_for("dir.png/readme"), None);
        assert_eq!(mime_for("main.rs"), None);
        assert_eq!(mime_for("Makefile"), None);
    }

    #[test]
    fn revision_colons() {
        assert!(!colon_outside_braces("HEAD~1"));
        assert!(!colon_outside_braces("main@{2026-11-08 10:00:00}"));
        assert!(colon_outside_braces("HEAD:dir"));
        assert!(colon_outside_braces(":/message"));
        assert!(is_stage(":2") && is_stage(":0"));
        assert!(!is_stage(":4") && !is_stage(":/x") && !is_stage(":") && !is_stage(":22"));
    }
}
