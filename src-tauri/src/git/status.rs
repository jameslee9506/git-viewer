//! Working tree status (`git status --porcelain=v2`).

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, Git};
use crate::git::repo;
use crate::model::{HeadInfo, StatusEntry, WorkingStatus};
use crate::watcher::{self, IgnoredPaths};
use std::path::Path;
use std::time::Instant;

/// Status of the worktree at `cwd` (any worktree of the repo).
///
/// When a watcher watches this worktree, the status also lists its gitignored paths
/// (`--ignored=matching`, about 5% slower) and hands them to the watcher, which then drops changes
/// below them. Ignored directories are listed whole, so this stays cheap with a big node_modules.
pub fn status(cwd: &Path) -> AppResult<WorkingStatus> {
    if !cwd.is_dir() {
        return Err(AppError::msg(format!("not a directory: {}", cwd.display())));
    }
    let watched = watcher::watched_root(cwd);
    let taken = Instant::now();
    let (out, git_dir) = std::thread::scope(|s| {
        let git_dir = s.spawn(|| Git::new(cwd).args(["rev-parse", "--absolute-git-dir"]).run());
        let out = Git::new(cwd)
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
                "--find-renames",
                "--ignore-submodules=dirty",
            ])
            .args(watched.is_some().then_some("--ignored=matching"))
            .run();
        let git_dir = git_dir.join().unwrap_or_else(|_| Err(AppError::msg("git rev-parse --absolute-git-dir failed")));
        (out, git_dir)
    });
    let mut ignored = IgnoredPaths::default();
    let mut st = parse(&out?, &mut ignored);
    if let Some(root) = watched {
        watcher::share_ignored(&root, taken, ignored);
    }
    st.cwd = cwd.to_string_lossy().into_owned();
    let git_dir = git_dir?;
    let git_dir = git_dir.strip_suffix(b"\n").unwrap_or(&git_dir[..]);
    if !git_dir.is_empty() {
        st.operation = repo::operation(Path::new(&lossy(git_dir)));
    }
    Ok(st)
}

/// Parse `git status --porcelain=v2 --branch -z` output (operation left None; cwd left empty).
pub fn parse_porcelain_v2(buf: &[u8]) -> WorkingStatus {
    parse(buf, &mut IgnoredPaths::default())
}

/// [`parse_porcelain_v2`] that also collects the paths of `! <path>` records into `ignored`.
fn parse(buf: &[u8], ignored: &mut IgnoredPaths) -> WorkingStatus {
    let mut st = WorkingStatus {
        cwd: String::new(),
        head: HeadInfo::default(),
        upstream: None,
        ahead: 0,
        behind: 0,
        staged: Vec::new(),
        unstaged: Vec::new(),
        untracked: Vec::new(),
        conflicted: Vec::new(),
        operation: None,
    };
    let mut records = buf.split(|b| *b == 0);
    while let Some(rec) = records.next() {
        match rec.first().copied() {
            Some(b'#') => header(&mut st, rec),
            Some(b'1') => {
                if let Some([_, xy, sub, _, _, _, _, _, path]) = fields::<9>(rec) {
                    changed(&mut st, xy, sub, path, None);
                }
            }
            Some(b'2') => {
                // The source path always follows as its own NUL-terminated field.
                let orig = records.next();
                if let Some([_, xy, sub, _, _, _, _, _, _, path]) = fields::<10>(rec) {
                    changed(&mut st, xy, sub, path, orig);
                }
            }
            Some(b'u') => {
                if let Some([_, xy, sub, _, _, _, _, _, _, _, path]) = fields::<11>(rec) {
                    st.conflicted.push(StatusEntry {
                        path: lossy(path),
                        old_path: None,
                        status: "U".into(),
                        conflict: Some(lossy(xy)),
                        submodule: is_submodule(sub),
                    });
                }
            }
            // A trailing `/` marks an untracked nested repository; it is kept as is.
            Some(b'?') if rec.len() > 2 => st.untracked.push(StatusEntry {
                path: lossy(&rec[2..]),
                old_path: None,
                status: "?".into(),
                conflict: None,
                submodule: false,
            }),
            // `--ignored=matching`; a trailing `/` marks a directory.
            Some(b'!') if rec.len() > 2 => ignored.insert(&rec[2..]),
            _ => {}
        }
    }
    st
}

/// `# branch.*` headers; unknown ones (e.g. `# stash N`) are ignored.
fn header(st: &mut WorkingStatus, rec: &[u8]) {
    let Some(rest) = rec.strip_prefix(b"# ") else { return };
    let (key, value) = match rest.iter().position(|b| *b == b' ') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    match key {
        b"branch.oid" => {
            if value == b"(initial)" {
                st.head.unborn = true;
            } else {
                st.head.oid = Some(lossy(value));
            }
        }
        b"branch.head" => {
            if value == b"(detached)" {
                st.head.detached = true;
            } else {
                st.head.branch = Some(lossy(value));
            }
        }
        b"branch.upstream" => st.upstream = Some(lossy(value)),
        // `+A -B`; `+? -?` (unknown) leaves zeros.
        b"branch.ab" => {
            for part in value.split(|b| *b == b' ') {
                match part.split_first() {
                    Some((&b'+', n)) => st.ahead = count(n),
                    Some((&b'-', n)) => st.behind = count(n),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Ordinary (`1`) and rename/copy (`2`) records: X goes to `staged`, Y to `unstaged`.
fn changed(st: &mut WorkingStatus, xy: &[u8], sub: &[u8], path: &[u8], orig: Option<&[u8]>) {
    let &[x, y] = xy else { return };
    let path = lossy(path);
    let submodule = is_submodule(sub);
    // The source path belongs to the side reporting R/C (`.R` = rename of an intent-to-add file).
    let source = |c: u8| if matches!(c, b'R' | b'C') { orig.map(lossy) } else { None };
    let entry = |path: String, c: u8| StatusEntry {
        path,
        old_path: source(c),
        status: letter(c),
        conflict: None,
        submodule,
    };
    if x != b'.' {
        st.staged.push(entry(path.clone(), x));
    }
    if y != b'.' {
        st.unstaged.push(entry(path, y));
    }
}

/// Split `rec` on the first `N - 1` spaces; the last field keeps the rest (paths may contain spaces).
fn fields<const N: usize>(rec: &[u8]) -> Option<[&[u8]; N]> {
    let empty: &[u8] = &[];
    let mut out = [empty; N];
    let mut it = rec.splitn(N, |b| *b == b' ');
    for slot in &mut out {
        *slot = it.next()?;
    }
    Some(out)
}

/// `<sub>` is `N...` for regular files and `S<c><m><u>` for submodules.
fn is_submodule(sub: &[u8]) -> bool {
    sub.first() == Some(&b'S')
}

fn letter(c: u8) -> String {
    char::from(c).to_string()
}

fn count(b: &[u8]) -> u32 {
    std::str::from_utf8(b).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `git status --porcelain=v2 --branch -z --untracked-files=all --find-renames
    // --ignore-submodules=dirty` output (git 2.48.1).

    /// Ahead/behind upstream; staged renames (one also modified), intent-to-add (plus its rename),
    /// type changes both ways, submodule commit change, untracked nested dirs, nested repo,
    /// names with spaces, TAB, LF and Korean.
    const ST: &[u8] = b"# branch.oid 0158b9d26427d87bbfcf439861b77641c18c00f0\0\
    # branch.head main\0\
    # branch.upstream origin/main\0\
    # branch.ab +2 -1\0\
    1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 397b4a7624e35fa60563a9c03b1213d93f7b6546 .gitignore\0\
    1 AM N... 000000 100644 100644 0000000000000000000000000000000000000000 d19f631813759dfb4e914ea3f5f183800273aa68 added-mod.txt\0\
    1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 d5f7fc3f74f7dec08280f370a975b112e8f60818 added.txt\0\
    1 .M N... 100644 100644 100644 587be6b4c3f93f93c489c0111bba5596147a26cb 587be6b4c3f93f93c489c0111bba5596147a26cb dir with space/f.txt\0\
    2 .R N... 100644 100644 100644 71ac1b5791204c80666ab1a4f9886b79e982739c 71ac1b5791204c80666ab1a4f9886b79e982739c R100 ita-dst.txt\0\
    ita-src.txt\0\
    1 .A N... 000000 000000 100644 0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 ita.txt\0\
    1 .M SC.. 160000 160000 160000 95401461360fae4d9ac81a259a5ab9e7dc593ad5 95401461360fae4d9ac81a259a5ab9e7dc593ad5 libs/sub\0\
    1 .T N... 120000 120000 100644 e1db6042c08861f1b576f06bef9563bf29f38d2a e1db6042c08861f1b576f06bef9563bf29f38d2a link\0\
    1 MM N... 100644 100644 100644 b5692e52ea4c05b80534a2959c7f5f31659a65b0 8f3b82bf308e6f4402bbffcd6af629b25bf8ea6b mm.txt\0\
    2 RM N... 100644 100644 100644 b00a0f1b675b8fc8b8aff62e13f2139842d025eb b00a0f1b675b8fc8b8aff62e13f2139842d025eb R100 renamed.txt\0\
    rename-me.txt\0\
    2 R. N... 100644 100644 100644 32442b73e095c5e9829fdc77f984492fb49b405d 32442b73e095c5e9829fdc77f984492fb49b405d R100 sp new.txt\0\
    sp old.txt\0\
    1 D. N... 100644 000000 000000 286c5f5776916d7d7d5849988ca9d83e722cf9c2 0000000000000000000000000000000000000000 staged-del.txt\0\
    1 T. N... 100644 120000 120000 b9bca019c83a65e6d717d0b6da86215f45dde1b3 e1db6042c08861f1b576f06bef9563bf29f38d2a will-be-link.txt\0\
    1 .D N... 100644 100644 000000 5c1ea48f59ade7e45cfc59889471e3ca29d0f892 5c1ea48f59ade7e45cfc59889471e3ca29d0f892 wt-del.txt\0\
    ? deep/er/est/u1.txt\0\
    ? deep/u2.txt\0\
    ? dir with space/new file.txt\0\
    ? nested/\0\
    ? new\nline.txt\0\
    ? tab\tname.txt\0\
    ? \xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc.txt\0";

    /// Merge with every unmerged kind (rename/rename gives DD + AU + UA), no upstream.
    const CF: &[u8] = b"# branch.oid 1aba81a594865a87b6c165e80275d5ef3eecc042\0\
    # branch.head main\0\
    1 .M N... 100644 100644 100644 2fa992c0b8b5c6acd2bdd4fa31de29d29799bdd5 2fa992c0b8b5c6acd2bdd4fa31de29d29799bdd5 keep.txt\0\
    u AA N... 000000 100644 100644 100644 0000000000000000000000000000000000000000 d72f0e2775d2f47a69586ac8b036430dd23e86fb a51d415711a5f4f15d42a0236edcc427e85f1a58 aa.txt\0\
    u UU N... 100644 100644 100644 100644 df967b96a579e45a18b8251732d16804b2e56a55 b19a1e93bec1317dc6097229e12afaffbfa74dc2 950b81b7eee953d050aa05a641f8e056c85dd1bd both mod.txt\0\
    u DU N... 100644 000000 100644 100644 df967b96a579e45a18b8251732d16804b2e56a55 0000000000000000000000000000000000000000 950b81b7eee953d050aa05a641f8e056c85dd1bd du.txt\0\
    u DD N... 100644 000000 000000 000000 0ec1772728d194cd451203c28b1d4184aa1575cc 0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 orig.txt\0\
    u AU N... 000000 100644 000000 100644 0000000000000000000000000000000000000000 0ec1772728d194cd451203c28b1d4184aa1575cc 0000000000000000000000000000000000000000 ours-name.txt\0\
    u UA N... 000000 000000 100644 100644 0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 0ec1772728d194cd451203c28b1d4184aa1575cc theirs-name.txt\0\
    u UD N... 100644 100644 000000 100644 df967b96a579e45a18b8251732d16804b2e56a55 b19a1e93bec1317dc6097229e12afaffbfa74dc2 0000000000000000000000000000000000000000 ud.txt\0\
    u UU N... 100644 100644 100644 100644 df967b96a579e45a18b8251732d16804b2e56a55 b19a1e93bec1317dc6097229e12afaffbfa74dc2 950b81b7eee953d050aa05a641f8e056c85dd1bd uu.txt\0";

    const UNBORN: &[u8] = b"# branch.oid (initial)\0\
    # branch.head main\0\
    1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 78981922613b2afb6025042ff6bd878ac1994e85 a.txt\0\
    ? b.txt\0";

    const DETACHED: &[u8] = b"# branch.oid c2c237a000c963ad9fb59950a4322c0744ef47a5\0\
    # branch.head (detached)\0\
    1 .M N... 100644 100644 100644 78981922613b2afb6025042ff6bd878ac1994e85 78981922613b2afb6025042ff6bd878ac1994e85 a.txt\0";

    /// Upstream deleted on the remote: `branch.upstream` without `branch.ab` (first records).
    const GONE: &[u8] = b"# branch.oid 0158b9d26427d87bbfcf439861b77641c18c00f0\0\
    # branch.head gone-br\0\
    # branch.upstream origin/gone-br\0\
    1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 397b4a7624e35fa60563a9c03b1213d93f7b6546 .gitignore\0";

    /// Submodule records: staged commit change, and (with --ignore-submodules=none) both sides.
    const SUBMODULE: &[u8] = b"1 M. S... 160000 160000 160000 95401461360fae4d9ac81a259a5ab9e7dc593ad5 fb73c1f5c4f039c0e64f45278bce0c05ae888d10 libs/sub\0\
    1 MM S.MU 160000 160000 160000 95401461360fae4d9ac81a259a5ab9e7dc593ad5 fb73c1f5c4f039c0e64f45278bce0c05ae888d10 libs/sub2\0";

    /// The same flags plus `--ignored=matching`, ignoring `node_modules/`, `target`, `*.log`,
    /// `build/` (holding the tracked `build/keep.txt`), `dist/*` but `!dist/.gitkeep`, `empty-ign/`,
    /// `nested/` (a nested repo) and `src/.gitignore` (via info/exclude); a staged rename whose
    /// source is named `! old.txt`.
    const IGNORED: &[u8] = b"# branch.oid 8162ceb0b427acd722eff3eace516e77df9e5dd0\0\
    # branch.head main\0\
    2 R. N... 100644 100644 100644 45b983be36b73c0788dc9cbcb76cbb80fc7bb057 45b983be36b73c0788dc9cbcb76cbb80fc7bb057 R100 new.txt\0\
    ! old.txt\0\
    ! app.log\0\
    ! build/.gitignore\0\
    ! build/gen.o\0\
    ! build/sub/\0\
    ! dist/main.123.js\0\
    ! empty-ign/\0\
    ! nested/\0\
    ! node_modules/\0\
    ! src/.gitignore\0\
    ! src/b.log\0\
    ! target/\0";

    fn rows(list: &[StatusEntry]) -> Vec<(&str, Option<&str>, &str, bool)> {
        list.iter().map(|e| (e.path.as_str(), e.old_path.as_deref(), e.status.as_str(), e.submodule)).collect()
    }

    #[test]
    fn branch_headers_and_ahead_behind() {
        let st = parse_porcelain_v2(ST);
        assert_eq!(st.head.oid.as_deref(), Some("0158b9d26427d87bbfcf439861b77641c18c00f0"));
        assert_eq!(st.head.branch.as_deref(), Some("main"));
        assert!(!st.head.detached && !st.head.unborn);
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert!(st.conflicted.is_empty());
        assert!(st.cwd.is_empty() && st.operation.is_none());
    }

    #[test]
    fn staged_and_unstaged_sides() {
        let st = parse_porcelain_v2(ST);
        assert_eq!(
            rows(&st.staged),
            vec![
                (".gitignore", None, "A", false),
                ("added-mod.txt", None, "A", false),
                ("added.txt", None, "A", false),
                ("mm.txt", None, "M", false),
                ("renamed.txt", Some("rename-me.txt"), "R", false),
                ("sp new.txt", Some("sp old.txt"), "R", false),
                ("staged-del.txt", None, "D", false),
                ("will-be-link.txt", None, "T", false),
            ]
        );
        assert_eq!(
            rows(&st.unstaged),
            vec![
                ("added-mod.txt", None, "M", false),
                ("dir with space/f.txt", None, "M", false),
                ("ita-dst.txt", Some("ita-src.txt"), "R", false),
                ("ita.txt", None, "A", false),
                ("libs/sub", None, "M", true),
                ("link", None, "T", false),
                ("mm.txt", None, "M", false),
                ("renamed.txt", None, "M", false),
                ("wt-del.txt", None, "D", false),
            ]
        );
        assert!(st.staged.iter().chain(&st.unstaged).all(|e| e.conflict.is_none()));
    }

    #[test]
    fn untracked_paths_kept_verbatim() {
        let st = parse_porcelain_v2(ST);
        let paths: Vec<&str> = st.untracked.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "deep/er/est/u1.txt",
                "deep/u2.txt",
                "dir with space/new file.txt",
                "nested/",
                "new\nline.txt",
                "tab\tname.txt",
                "새 파일.txt",
            ]
        );
        assert!(st.untracked.iter().all(|e| e.status == "?" && e.old_path.is_none() && !e.submodule));
    }

    #[test]
    fn conflicts_of_every_kind() {
        let st = parse_porcelain_v2(CF);
        let got: Vec<(&str, &str, Option<&str>)> =
            st.conflicted.iter().map(|e| (e.path.as_str(), e.status.as_str(), e.conflict.as_deref())).collect();
        assert_eq!(
            got,
            vec![
                ("aa.txt", "U", Some("AA")),
                ("both mod.txt", "U", Some("UU")),
                ("du.txt", "U", Some("DU")),
                ("orig.txt", "U", Some("DD")),
                ("ours-name.txt", "U", Some("AU")),
                ("theirs-name.txt", "U", Some("UA")),
                ("ud.txt", "U", Some("UD")),
                ("uu.txt", "U", Some("UU")),
            ]
        );
        assert_eq!(rows(&st.unstaged), vec![("keep.txt", None, "M", false)]);
        assert!(st.staged.is_empty() && st.untracked.is_empty());
        assert_eq!(st.upstream, None);
        assert_eq!((st.ahead, st.behind), (0, 0));
    }

    #[test]
    fn unborn_branch() {
        let st = parse_porcelain_v2(UNBORN);
        assert!(st.head.unborn && !st.head.detached);
        assert_eq!(st.head.oid, None);
        assert_eq!(st.head.branch.as_deref(), Some("main"));
        assert_eq!(rows(&st.staged), vec![("a.txt", None, "A", false)]);
        assert_eq!(rows(&st.untracked), vec![("b.txt", None, "?", false)]);
    }

    #[test]
    fn detached_head() {
        let st = parse_porcelain_v2(DETACHED);
        assert!(st.head.detached && !st.head.unborn);
        assert_eq!(st.head.branch, None);
        assert_eq!(st.head.oid.as_deref(), Some("c2c237a000c963ad9fb59950a4322c0744ef47a5"));
        assert_eq!(rows(&st.unstaged), vec![("a.txt", None, "M", false)]);
    }

    #[test]
    fn gone_upstream_has_no_counts() {
        let st = parse_porcelain_v2(GONE);
        assert_eq!(st.head.branch.as_deref(), Some("gone-br"));
        assert_eq!(st.upstream.as_deref(), Some("origin/gone-br"));
        assert_eq!((st.ahead, st.behind), (0, 0));
        assert_eq!(rows(&st.staged), vec![(".gitignore", None, "A", false)]);
    }

    #[test]
    fn submodule_flags_on_both_sides() {
        let st = parse_porcelain_v2(SUBMODULE);
        assert_eq!(rows(&st.staged), vec![("libs/sub", None, "M", true), ("libs/sub2", None, "M", true)]);
        assert_eq!(rows(&st.unstaged), vec![("libs/sub2", None, "M", true)]);
    }

    #[test]
    fn ignored_records_feed_the_watcher_only() {
        let mut ignored = IgnoredPaths::default();
        let st = parse(IGNORED, &mut ignored);
        assert_eq!(rows(&st.staged), vec![("new.txt", Some("! old.txt"), "R", false)]);
        assert!(st.unstaged.is_empty() && st.untracked.is_empty() && st.conflicted.is_empty());
        let covers = |p: &str| ignored.covers(Path::new(p));
        assert!(!covers("old.txt")); // the rename source field, not a record
        assert!(covers("app.log") && covers("src/b.log") && covers("build/gen.o") && covers("dist/main.123.js"));
        assert!(covers("node_modules/a/b.js") && covers("target/debug/x") && covers("build/sub/x.o"));
        assert!(covers("empty-ign/x") && covers("nested/f"));
        assert!(!covers("build/keep.txt") && !covers("src/a.rs") && !covers("dist/.gitkeep") && !covers("new.txt"));
        assert!(!covers("src/.gitignore")); // ignored, yet git reads it
        assert_eq!(rows(&parse_porcelain_v2(IGNORED).staged), rows(&st.staged));
    }

    #[test]
    fn tolerates_unknown_headers_and_noise() {
        let st = parse_porcelain_v2(b"# branch.oid abc\0# branch.ab +? -?\0# stash 3\0! ignored.log\0x junk\0? \0");
        assert_eq!(st.head.oid.as_deref(), Some("abc"));
        assert_eq!((st.ahead, st.behind), (0, 0));
        assert!(st.untracked.is_empty() && st.staged.is_empty() && st.unstaged.is_empty());
        let empty = parse_porcelain_v2(b"");
        assert!(empty.head.oid.is_none() && empty.staged.is_empty());
    }
}
