//! Path attributes (`git check-attr`): which listed paths Git LFS tracks.

use crate::git::cmd::{lossy, split_records, Git};
use std::collections::HashSet;
use std::path::Path;

/// Paths whose `filter` attribute is `lfs` (Git LFS tracks them: their blobs are pointer files), by
/// the `.gitattributes` of `source` (a tree-ish: a commit's own rules) or, without one, of the working
/// tree at `cwd` — plus `info/attributes` and the global attributes file either way. One
/// `git check-attr -z --stdin [--source=<rev>] filter` run with the paths on stdin; a git too old for
/// `--source` (< 2.40) answers from the working tree instead. Never fails: a failed run marks nothing.
pub fn lfs_paths<'a>(cwd: &Path, source: Option<&str>, paths: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    if input.is_empty() {
        return HashSet::new();
    }
    let run = |source: Option<&str>| {
        let git = Git::new(cwd).args(["check-attr", "-z", "--stdin"]);
        let git = match source {
            Some(rev) => git.arg(format!("--source={rev}")),
            None => git,
        };
        git.arg("filter").stdin(input.clone()).run()
    };
    let out = match run(source) {
        Ok(out) => out,
        Err(_) if source.is_some() => match run(None) {
            Ok(out) => out,
            Err(_) => return HashSet::new(),
        },
        Err(_) => return HashSet::new(),
    };
    parse_check_attr(&out)
}

/// Parse `git check-attr -z` output (`path\0attr\0value\0` records): the paths whose value is `lfs`.
pub fn parse_check_attr(buf: &[u8]) -> HashSet<String> {
    split_records(buf, 0).chunks_exact(3).filter(|r| r[2] == b"lfs").map(|r| lossy(r[0])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_attr_lfs_paths() {
        // `printf 'a.png\0b.txt\0d/c.png\0with space.png\0' | git check-attr -z --stdin filter`
        let out =
            b"a.png\0filter\0lfs\0b.txt\0filter\0unspecified\0d/c.png\0filter\0lfs\0with space.png\0filter\0lfs\0";
        let lfs = parse_check_attr(out);
        assert_eq!(lfs.len(), 3);
        assert!(lfs.contains("a.png") && lfs.contains("d/c.png") && lfs.contains("with space.png"));
        assert!(!lfs.contains("b.txt"));
        assert!(parse_check_attr(b"").is_empty());
        // A different filter (or an unset / unspecified one) is not LFS.
        assert!(parse_check_attr(b"x.bin\0filter\0crypt\0y.bin\0filter\0unset\0").is_empty());
    }
}
