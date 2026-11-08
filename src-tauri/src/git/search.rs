//! git-backed commit searches (message grep, path, pickaxe) over the same revision set as the graph.

use crate::error::{AppError, AppResult};
use crate::git::cmd::Git;
use crate::git::log;
use crate::model::{GraphOrder, SearchMode};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// Searches are stopped after this long (a pickaxe over a big history can take minutes).
const TIMEOUT: Duration = Duration::from_secs(30);

/// Oids of commits matching `text` in `mode` (not Quick), reachable from `tips`.
///
/// `tips` are the `--stdin` revisions, one per line: the graph's tip oids, optionally followed by
/// `^<oid>` exclusions that bound the walk to the loaded commits. `order` and `limit` do not change the
/// matched set and are not used: the caller maps the oids to graph rows (dropping commits outside the
/// graph) and sorts them.
pub fn git_search(
    cwd: &Path,
    tips: &[String],
    order: GraphOrder,
    limit: u32,
    mode: SearchMode,
    text: &str,
    cancel: Option<Arc<AtomicBool>>,
) -> AppResult<Vec<String>> {
    let _ = (order, limit);
    let (flags, pathspec) = search_args(mode, text)?;
    // A NUL can't be passed as an argument and matches nothing; no positive tip means no history.
    if text.is_empty() || text.contains('\0') || !tips.iter().any(|t| !t.is_empty() && !t.starts_with('^')) {
        return Ok(Vec::new());
    }
    let input = log::rev_input(tips);
    let mut git = Git::new(cwd)
        .args(["log", "--format=%H", "--no-decorate", "--ignore-missing"])
        .args(&flags)
        .args(["--stdin", "--"]);
    if let Some(p) = pathspec {
        git = git.arg(p);
    }
    if let Some(flag) = cancel {
        git = git.cancel_on(flag);
    }
    let out = git.stdin(input).timeout(TIMEOUT).run()?;
    Ok(parse_oids(&out))
}

/// Path search flags. A commit matches when its diff against its first parent (root: the empty tree),
/// the file list the details panel shows, touches a matching path.
/// - `--full-history`: the default simplification walks only a TREESAME parent of a merge and misses
///   the commits behind the other parents (`-s ours` merges, cherry-picked or reverted changes,
///   conflicts resolved to the other side).
/// - `--diff-merges=first-parent --diff-filter=ACDMRTUXB`: keep only commits whose first-parent diff
///   within the pathspec is non-empty. `--full-history` alone also lists every merge whose other
///   parent has a different version of the path (stale branches, `-s ours`).
/// - `--no-renames`: renames never empty that diff, and detecting them reads blobs (slow on big
///   commits; fatal in a blob-less partial clone, as lazy fetching is disabled).
/// - `--ignore-submodules=none`: `diff.ignoreSubmodules` or `submodule.<name>.ignore=all` would hide
///   submodule pointer changes from the diff.
/// - `--root`: a user's `log.showRoot=false` would skip the root commits' diffs.
/// - `--no-follow`: a user's `log.follow=true` would otherwise follow the single pathspec.
/// - `--no-patch`: `--diff-merges` turns on patch output; stdout stays one oid per line.
const PATH_FLAGS: [&str; 8] = [
    "--no-follow",
    "--full-history",
    "--diff-merges=first-parent",
    "--diff-filter=ACDMRTUXB",
    "--no-renames",
    "--ignore-submodules=none",
    "--root",
    "--no-patch",
];

/// `git log` flags for `mode`, plus the pathspec for path searches.
fn search_args(mode: SearchMode, text: &str) -> AppResult<(Vec<String>, Option<String>)> {
    Ok(match mode {
        SearchMode::Quick => return Err(AppError::msg("quick search runs in memory")),
        SearchMode::Message => (vec!["-i".into(), "-F".into(), format!("--grep={text}")], None),
        SearchMode::Path => (PATH_FLAGS.map(String::from).to_vec(), Some(path_pattern(text))),
        // No textconv: a pickaxe would otherwise run the user's conversion filters on every blob.
        // `--root`: a user's `log.showRoot=false` would skip the root commits' diffs, and with them
        // every root-commit hit.
        SearchMode::Content => (vec![format!("-S{text}"), "--no-textconv".into(), "--root".into()], None),
    })
}

/// Case-insensitive pathspec matching paths that contain `text`, or the glob `text` itself when it
/// has glob characters. Globs read like `.gitignore` patterns: without a `/` they match at any depth
/// (`*.rs` → `**/*.rs`, where `**/` also matches the top level); a leading `/` anchors them at the
/// root (git rejects a pathspec starting with `/`).
pub fn path_pattern(text: &str) -> String {
    if !text.contains(['*', '?', '[']) {
        return format!(":(icase)*{text}*");
    }
    match text.strip_prefix('/') {
        Some(rooted) => format!(":(glob,icase){}", rooted.trim_start_matches('/')),
        None if text.contains('/') => format!(":(glob,icase){text}"),
        None => format!(":(glob,icase)**/{text}"),
    }
}

/// Oids of `--format=%H` output (one per line).
pub fn parse_oids(out: &[u8]) -> Vec<String> {
    out.split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| !l.is_empty())
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_patterns() {
        assert_eq!(path_pattern("lib/foo"), ":(icase)*lib/foo*");
        assert_eq!(path_pattern("/lib"), ":(icase)*/lib*");
        assert_eq!(path_pattern("src/**/*.txt"), ":(glob,icase)src/**/*.txt");
        assert_eq!(path_pattern("a?.rs"), ":(glob,icase)**/a?.rs");
        assert_eq!(path_pattern("[ab].rs"), ":(glob,icase)**/[ab].rs");
        assert_eq!(path_pattern("*.rs"), ":(glob,icase)**/*.rs");
        assert_eq!(path_pattern("**/*.rs"), ":(glob,icase)**/*.rs");
        assert_eq!(path_pattern("/*.rs"), ":(glob,icase)*.rs");
        assert_eq!(path_pattern("//src/*.rs"), ":(glob,icase)src/*.rs");
    }

    #[test]
    fn args_per_mode() {
        let (f, p) = search_args(SearchMode::Message, "fix: it").unwrap();
        assert_eq!(f, ["-i", "-F", "--grep=fix: it"]);
        assert_eq!(p, None);
        let (f, p) = search_args(SearchMode::Path, "README").unwrap();
        assert_eq!(
            f,
            [
                "--no-follow",
                "--full-history",
                "--diff-merges=first-parent",
                "--diff-filter=ACDMRTUXB",
                "--no-renames",
                "--ignore-submodules=none",
                "--root",
                "--no-patch"
            ]
        );
        assert_eq!(p.as_deref(), Some(":(icase)*README*"));
        let (f, p) = search_args(SearchMode::Content, "-x").unwrap();
        assert_eq!(f, ["-S-x", "--no-textconv", "--root"]);
        assert_eq!(p, None);
        assert!(search_args(SearchMode::Quick, "x").is_err());
    }

    #[test]
    fn parses_oid_lines() {
        let out = b"6bd3ead629c2c36c7e2c5053eb5fff91cb360277\n77fa06895f705c2cd22d3f61383adcb6de0e2490\n";
        assert_eq!(
            parse_oids(out),
            ["6bd3ead629c2c36c7e2c5053eb5fff91cb360277", "77fa06895f705c2cd22d3f61383adcb6de0e2490"]
        );
        assert!(parse_oids(b"").is_empty());
        assert_eq!(parse_oids(b"abcd\r\n\n"), ["abcd"]);
    }

    #[test]
    fn nothing_to_walk_returns_empty_without_git() {
        let tips = vec!["^6bd3ead629c2c36c7e2c5053eb5fff91cb360277".to_string()];
        let none = Path::new("/nonexistent-git-viewer-dir");
        assert!(git_search(none, &tips, GraphOrder::Date, 0, SearchMode::Message, "x", None).unwrap().is_empty());
        assert!(git_search(none, &[], GraphOrder::Date, 0, SearchMode::Content, "x", None).unwrap().is_empty());
        let tip = vec!["6bd3ead629c2c36c7e2c5053eb5fff91cb360277".to_string()];
        assert!(git_search(none, &tip, GraphOrder::Date, 0, SearchMode::Path, "a\0b", None).unwrap().is_empty());
        assert!(git_search(none, &tip, GraphOrder::Date, 0, SearchMode::Quick, "x", None).is_err());
    }
}
