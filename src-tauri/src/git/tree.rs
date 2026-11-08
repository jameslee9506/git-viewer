//! Tree listing at a revision (`git ls-tree`).

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, split_records, Git};
use crate::model::TreeEntry;
use std::path::Path;

/// Entries of directory `path` ('' = root) at `rev`; `recursive` lists every blob (and submodule)
/// below it.
pub fn list_tree(repo: &Path, rev: &str, path: &str, recursive: bool) -> AppResult<Vec<TreeEntry>> {
    if rev.is_empty() || rev.starts_with('-') || rev.contains('\0') {
        return Err(AppError::msg(format!("invalid revision: {rev}")));
    }
    if path.contains('\0') {
        return Err(AppError::msg(format!("invalid path: {path}")));
    }
    if !repo.is_dir() {
        return Err(AppError::msg(format!("not a directory: {}", repo.display())));
    }
    let dir = path.trim_matches('/');
    // `<rev>:<dir>` lists the subtree itself, so names come out relative to `dir`.
    let object = if dir.is_empty() { rev.to_string() } else { format!("{rev}:{dir}") };
    let mut git = Git::new(repo).args(["ls-tree", "-z", "-l"]);
    if recursive {
        git = git.arg("-r");
    }
    let out = git.arg(object).literal_paths().run()?;
    Ok(parse_ls_tree(&out, dir))
}

/// Parse `git ls-tree -z -l [-r]` output of directory `dir` ('' = root), whose names are relative
/// to it. Sorted like a file explorer: at every level directories first, then names
/// case-insensitively. Sizes are None for trees, submodules and blobs whose size is unknown
/// (`BAD`: missing in a partial clone).
pub fn parse_ls_tree(buf: &[u8], dir: &str) -> Vec<TreeEntry> {
    let mut keyed: Vec<(Vec<u8>, TreeEntry)> = Vec::new();
    for rec in split_records(buf, 0) {
        // `<mode> SP <type> SP <oid> SP+ <size>\t<name>`: names may contain TABs, the metadata can't.
        let Some(tab) = rec.iter().position(|b| *b == b'\t') else { continue };
        let mut meta = rec[..tab].split(u8::is_ascii_whitespace).filter(|s| !s.is_empty());
        let (Some(mode), Some(kind), Some(oid)) = (meta.next(), meta.next(), meta.next()) else { continue };
        let size = match kind {
            b"blob" => meta.next().and_then(|s| std::str::from_utf8(s).ok()?.parse().ok()),
            _ => None,
        };
        let rel = lossy(&rec[tab + 1..]);
        let key = sort_key(&rel, kind == b"tree");
        let name = rel.rsplit('/').next().unwrap_or_default().to_string();
        let path = if dir.is_empty() { rel } else { format!("{dir}/{rel}") };
        keyed.push((key, TreeEntry { name, path, kind: lossy(kind), mode: lossy(mode), oid: lossy(oid), size }));
    }
    keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    keyed.into_iter().map(|(_, e)| e).collect()
}

/// Byte key that orders paths component by component: a rank (directories before files), the
/// lowercased name, then NUL so that a name sorts before its longer extensions.
fn sort_key(rel: &str, is_tree: bool) -> Vec<u8> {
    let lower = rel.to_lowercase();
    let mut key = Vec::with_capacity(lower.len() + 8);
    let mut parts = lower.split('/').peekable();
    while let Some(part) = parts.next() {
        let file = parts.peek().is_none() && !is_tree;
        key.push(if file { 2 } else { 1 });
        key.extend_from_slice(part.as_bytes());
        key.push(0);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `git ls-tree -z -l [-r]` output (git 2.48.1).

    /// Root: case variants, a name starting with `-`, symlink, executable, TAB and Korean names.
    const ROOT: &[u8] = b"100644 blob a2544f7ec3007899167de1fef481a5a0fd63fa41       5\t-dash.txt\0\
    100644 blob 4563368d112dda5f87a6df6ae79d72b791661377      66\t.gitmodules\0\
    100644 blob 223b7836fb19fdf64ba2d3cd6173c6a283141f78       2\tB.txt\0\
    040000 tree cf67e9ef3a0fc6d858423fc177f2fbbe985a6f17       -\tC\0\
    100644 blob 78981922613b2afb6025042ff6bd878ac1994e85       2\ta.txt\0\
    040000 tree f8f7aefc2900a3d737cea9eee45729fd55761e1a       -\tb\0\
    040000 tree 1d31567642c78f23e879aeb3c8bc484f8e14d838       -\tdir with space\0\
    120000 blob 8d14cbf983b3fad683171c9418998d9f68340823       5\tlink\0\
    040000 tree 840eefd8ff626d24719859a745f2981abcc293eb       -\tmods\0\
    100755 blob 86daf54cf5c47c1e43ed4a8f31094f508b3d6264       4\trun.sh\0\
    040000 tree 1957cf7869f27d599dc9b469e35f962a828f93cc       -\tsrc\0\
    100644 blob 718f4d2ff533cf8ead8d3556cf43912bd245fbc4       2\ttab\tname.txt\0\
    100644 blob 561f00b70bfee14952f82c8704bf7249603e946c    2000\tzeros.bin\0\
    040000 tree 7cfc7b7fc789a6c73f28d7b31d4836131db632c5       -\t\xed\x95\x9c\xea\xb8\x80\xed\x8f\xb4\xeb\x8d\x94\0";

    /// `HEAD:dir with space`.
    const DIR: &[u8] = b"040000 tree 3c7789a8d9c4ef1319e35989ea2ee85b1ff18716       -\tinner\0\
    100644 blob 975fbec8256d3e8a3797e7a3611380f27c49f4ac       2\ttop.txt\0";

    /// `HEAD:mods` holding a submodule.
    const MODS: &[u8] = b"160000 commit 95401461360fae4d9ac81a259a5ab9e7dc593ad5       -\tsub mod\0";

    /// `-r HEAD` (git order: plain byte order of full paths).
    const REC_ROOT: &[u8] = b"100644 blob a2544f7ec3007899167de1fef481a5a0fd63fa41       5\t-dash.txt\0\
    100644 blob 4563368d112dda5f87a6df6ae79d72b791661377      66\t.gitmodules\0\
    100644 blob 223b7836fb19fdf64ba2d3cd6173c6a283141f78       2\tB.txt\0\
    100644 blob f2ad6c76f0115a6ba5b00456a849810e7ec0af20       2\tC/c.txt\0\
    100644 blob 78981922613b2afb6025042ff6bd878ac1994e85       2\ta.txt\0\
    100644 blob 61780798228d17af2d34fce4cfbdf35556832472       2\tb/b.txt\0\
    100644 blob 587be6b4c3f93f93c489c0111bba5596147a26cb       2\tdir with space/inner/f g.txt\0\
    100644 blob 975fbec8256d3e8a3797e7a3611380f27c49f4ac       2\tdir with space/top.txt\0\
    120000 blob 8d14cbf983b3fad683171c9418998d9f68340823       5\tlink\0\
    160000 commit 95401461360fae4d9ac81a259a5ab9e7dc593ad5       -\tmods/sub mod\0\
    100755 blob 86daf54cf5c47c1e43ed4a8f31094f508b3d6264       4\trun.sh\0\
    100644 blob 4bcfe98e640c8284511312660fb8709b0afa888e       2\tsrc/lib/deep/z.rs\0\
    100644 blob 1f9d725a9de833a65966881dce2e907b86e72c5e       2\tsrc/lib/mod.rs\0\
    100644 blob 4286f428e3b19fe84de503916ce0e7dc8deefea1       2\tsrc/main.rs\0\
    100644 blob 718f4d2ff533cf8ead8d3556cf43912bd245fbc4       2\ttab\tname.txt\0\
    100644 blob 561f00b70bfee14952f82c8704bf7249603e946c    2000\tzeros.bin\0\
    100644 blob 71c5dd798c6a9a06fdc139da2e0d3d8ba8b0772a       4\t\xed\x95\x9c\xea\xb8\x80\xed\x8f\xb4\xeb\x8d\x94/\xed\x8c\x8c\xec\x9d\xbc.txt\0";

    /// `-r HEAD:src`.
    const REC_SRC: &[u8] = b"100644 blob 4bcfe98e640c8284511312660fb8709b0afa888e       2\tlib/deep/z.rs\0\
    100644 blob 1f9d725a9de833a65966881dce2e907b86e72c5e       2\tlib/mod.rs\0\
    100644 blob 4286f428e3b19fe84de503916ce0e7dc8deefea1       2\tmain.rs\0";

    /// Partial clone (`--filter=blob:limit=1k`, lazy fetch disabled): the missing blob's size is `BAD`.
    const PARTIAL: &[u8] = b"100644 blob 069cfbe4e7a1224f89dcc34d40c2264dbcb5aa11     BAD\tbig.bin\0\
    100644 blob ac790413e2d7a26c3767e78c57bb28716686eebc       6\tsmall.txt\0";

    fn names(v: &[TreeEntry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    fn paths(v: &[TreeEntry]) -> Vec<&str> {
        v.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn root_trees_first_then_case_insensitive() {
        let v = parse_ls_tree(ROOT, "");
        assert_eq!(
            names(&v),
            vec![
                "b", "C", "dir with space", "mods", "src", "한글폴더", "-dash.txt", ".gitmodules", "a.txt", "B.txt", "link",
                "run.sh", "tab\tname.txt", "zeros.bin",
            ]
        );
        assert_eq!(names(&v), paths(&v));
        let get = |n: &str| v.iter().find(|e| e.name == n).unwrap();
        let src = get("src");
        assert_eq!((src.kind.as_str(), src.mode.as_str(), src.size), ("tree", "040000", None));
        assert_eq!(src.oid, "1957cf7869f27d599dc9b469e35f962a828f93cc");
        let link = get("link");
        assert_eq!((link.kind.as_str(), link.mode.as_str(), link.size), ("blob", "120000", Some(5)));
        assert_eq!((get("run.sh").mode.as_str(), get("run.sh").size), ("100755", Some(4)));
        assert_eq!(get("zeros.bin").size, Some(2000));
        assert_eq!(get("tab\tname.txt").oid, "718f4d2ff533cf8ead8d3556cf43912bd245fbc4");
    }

    #[test]
    fn nested_dir_paths_are_prefixed() {
        let v = parse_ls_tree(DIR, "dir with space");
        assert_eq!(names(&v), vec!["inner", "top.txt"]);
        assert_eq!(paths(&v), vec!["dir with space/inner", "dir with space/top.txt"]);
        assert_eq!((v[0].kind.as_str(), v[1].size), ("tree", Some(2)));
    }

    #[test]
    fn submodule_entry() {
        let v = parse_ls_tree(MODS, "mods");
        assert_eq!(v.len(), 1);
        let e = &v[0];
        assert_eq!((e.name.as_str(), e.path.as_str(), e.kind.as_str(), e.mode.as_str()), ("sub mod", "mods/sub mod", "commit", "160000"));
        assert_eq!((e.oid.as_str(), e.size), ("95401461360fae4d9ac81a259a5ab9e7dc593ad5", None));
    }

    #[test]
    fn recursive_root_in_explorer_order() {
        let v = parse_ls_tree(REC_ROOT, "");
        assert_eq!(
            paths(&v),
            vec![
                "b/b.txt",
                "C/c.txt",
                "dir with space/inner/f g.txt",
                "dir with space/top.txt",
                "mods/sub mod",
                "src/lib/deep/z.rs",
                "src/lib/mod.rs",
                "src/main.rs",
                "한글폴더/파일.txt",
                "-dash.txt",
                ".gitmodules",
                "a.txt",
                "B.txt",
                "link",
                "run.sh",
                "tab\tname.txt",
                "zeros.bin",
            ]
        );
        assert_eq!(v[2].name, "f g.txt");
        assert_eq!((v[4].name.as_str(), v[4].kind.as_str(), v[4].size), ("sub mod", "commit", None));
        assert_eq!(v[8].name, "파일.txt");
    }

    #[test]
    fn recursive_subdir_is_prefixed() {
        let v = parse_ls_tree(REC_SRC, "src");
        assert_eq!(paths(&v), vec!["src/lib/deep/z.rs", "src/lib/mod.rs", "src/main.rs"]);
        assert_eq!(names(&v), vec!["z.rs", "mod.rs", "main.rs"]);
    }

    #[test]
    fn unknown_blob_size() {
        let v = parse_ls_tree(PARTIAL, "");
        assert_eq!(v.iter().map(|e| (e.name.as_str(), e.size)).collect::<Vec<_>>(), vec![("big.bin", None), ("small.txt", Some(6))]);
        assert!(parse_ls_tree(b"", "").is_empty());
    }

    #[test]
    fn shorter_names_and_case_ties_are_stable() {
        let buf = b"100644 blob 1111111111111111111111111111111111111111       1\tab.txt\0\
            100644 blob 2222222222222222222222222222222222222222       1\ta\0\
            100644 blob 3333333333333333333333333333333333333333       1\tA\0\
            040000 tree 4444444444444444444444444444444444444444       -\tab\0";
        assert_eq!(names(&parse_ls_tree(buf, "")), vec!["ab", "A", "a", "ab.txt"]);
    }
}
