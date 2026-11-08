//! Single-file structured diffs.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, Git};
use crate::git::commit::{relative_path, validate_oid, validate_path, validate_rev, with_head_tree, worktree_dir};
use crate::model::{DiffLine, DiffRequest, DiffTarget, FileDiff, Hunk};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Default `DiffRequest.max_bytes`: diff text above this size is returned as `too_large` without hunks.
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Bytes inspected for NUL to classify content as binary (git's heuristic).
const BINARY_SNIFF: u64 = 8000;

/// Longest line prefix kept while counting an oversized patch: a line's first bytes classify it.
const COUNT_LINE_CAP: usize = 64 * 1024;

/// Structured diff of one file for any target. `repo` is the opened repo root (worktree targets carry their own cwd).
///
/// `max_bytes` None = 8 MiB, 0 = unlimited; a larger patch comes back `too_large` with headers and line
/// counts but no hunks. The patch is streamed from git and buffered only up to the limit, so memory stays
/// near it whatever the patch size (`PatchSink`). An untracked nested repository (path ending in `/`)
/// yields only the header line `nested repository`. `old_path` is used for tree, stash, staged, unstaged
/// and worktree targets (renames).
///
/// Working-tree targets (staged, unstaged, conflicted, worktree, untracked) are refetched on every
/// worktree change and only the newest response is used: a newer request for the same repository, target
/// and file cancels one still running, which fails with `cancelled` (`LiveRun`). Refreshes while a huge
/// file keeps changing thus never stack git processes.
pub fn file_diff(repo: &Path, req: &DiffRequest) -> AppResult<FileDiff> {
    validate_path(&req.path)?;
    // Renames exist between trees, between a tree (HEAD or any commit) and the index or the files on disk,
    // and between the index and the worktree for intent-to-add files (status `2 .R`):
    // `diff-files -M -- <old> <new>` shows those as a rename too.
    let renames = !matches!(req.target, DiffTarget::Conflicted { .. } | DiffTarget::Untracked { .. });
    let old_path = req.old_path.as_deref().filter(|o| renames && *o != req.path);
    if let Some(o) = old_path {
        validate_path(o)?;
    }
    let max = match req.max_bytes {
        None => DEFAULT_MAX_BYTES,
        Some(0) => u64::MAX,
        Some(n) => n,
    };
    let flags = diff_flags(req);
    let paths: Vec<&str> = old_path.into_iter().chain([req.path.as_str()]).collect();
    let live = is_live(&req.target).then(|| LiveRun::begin(repo, req));
    let cancel = live.as_ref().map(|l| l.cancel.clone());
    let patch = |git: Git| run_patch(git, &paths, max, cancel.clone());
    let tree_diff = |cwd: &Path| Git::new(cwd).arg("diff-tree").args(&flags);

    let mut diff = match &req.target {
        DiffTarget::Commit { oid, parent } => {
            validate_oid(oid)?;
            match parent {
                Some(p) => {
                    validate_oid(p)?;
                    patch(tree_diff(repo).args([p, oid]))?
                }
                // One process for root, normal and merge commits alike (vs the first parent).
                None => patch(tree_diff(repo).args(["--root", "--no-commit-id", "--diff-merges=first-parent", oid]))?,
            }
        }
        DiffTarget::Range { from, to } => {
            validate_rev(from)?;
            validate_rev(to)?;
            patch(tree_diff(repo).args([from, to]))?
        }
        DiffTarget::Stash { oid } => {
            validate_oid(oid)?;
            patch(tree_diff(repo).arg(format!("{oid}^1")).arg(oid))?
        }
        DiffTarget::StashUntracked { oid } => {
            validate_oid(oid)?;
            patch(tree_diff(repo).args(["--root", "--no-commit-id"]).arg(format!("{oid}^3")))?
        }
        DiffTarget::Staged { cwd } => {
            let cwd = worktree_dir(cwd)?;
            with_head_tree(cwd, |base| patch(Git::new(cwd).args(["diff-index", "--cached"]).args(&flags).arg(base)))?
        }
        DiffTarget::Unstaged { cwd } => patch(Git::new(worktree_dir(cwd)?).arg("diff-files").args(&flags))?,
        DiffTarget::Conflicted { cwd } => patch(Git::new(worktree_dir(cwd)?).arg("diff-files").args(&flags).arg("-2"))?,
        // Without `--cached`: the commit's tree vs the files on disk (stat-only changes print nothing).
        DiffTarget::Worktree { cwd, oid } => {
            validate_oid(oid)?;
            patch(Git::new(worktree_dir(cwd)?).arg("diff-index").args(&flags).arg(oid))?
        }
        DiffTarget::Untracked { cwd } => untracked_diff(worktree_dir(cwd)?, &req.path, &flags, max, cancel.clone())?,
    };
    diff.path = req.path.clone();
    diff.old_path = old_path.map(str::to_string);
    Ok(diff)
}

/// Parse unified diff output for a single file (`diff --git` header, extended headers, hunks).
///
/// Lines before the first `diff ` line (commit id, `* Unmerged path …`) are ignored and only the first
/// file section is used, except that a type change (git prints a deletion and a creation section with
/// identical `diff --git` lines) is merged into one diff. Hunks are consumed by their line counts, so
/// content lines that look like headers (`--- x`, `+++ y`) are safe. Combined diffs (`diff --cc`) are
/// projected onto the first parent. All-zero blob ids (missing side) are reported as None.
pub fn parse_unified(buf: &[u8]) -> FileDiff {
    Parser::new(true).parse(buf)
}

fn diff_flags(req: &DiffRequest) -> Vec<String> {
    let mut flags: Vec<String> =
        ["-p", "--no-color", "--no-ext-diff", "--no-textconv", "--full-index", "--submodule=short"]
            .into_iter()
            .map(String::from)
            .collect();
    flags.push(format!("-U{}", req.context.unwrap_or(3)));
    if req.ignore_whitespace {
        flags.push("-w".into());
    }
    flags.push("-M".into());
    flags
}

/// Run a patch-producing git command on `paths`, streaming its output into a `PatchSink`.
///
/// `paths` = `[old, new]` for a rename: git prints a section per path when it does not pair them (a
/// `--follow` copy whose source changed in the same commit, `C` in the history), sorted by path, and only
/// the first section is used. An orderfile on stdin lists the requested (new) path first.
fn run_patch(git: Git, paths: &[&str], max: u64, cancel: Option<Arc<AtomicBool>>) -> AppResult<FileDiff> {
    let mut git = git;
    if let [_, path] = paths {
        if let Some(pattern) = order_pattern(path) {
            git = git.arg("-O/dev/stdin").stdin(format!("{pattern}\n").into_bytes());
        }
    }
    let mut git = git.arg("--").args(paths).literal_paths();
    if let Some(flag) = cancel {
        git = git.cancel_on(flag);
    }
    let mut patch = PatchSink::new(max);
    git.run_streaming(|chunk| patch.push(chunk))?;
    Ok(patch.finish())
}

/// An orderfile line (`-O`: one wildmatch pattern per line, `#` starts a comment) matching exactly `path`;
/// None for a name with a line feed.
fn order_pattern(path: &str) -> Option<String> {
    if path.contains('\n') {
        return None;
    }
    let mut pattern = String::with_capacity(path.len() + 2);
    for (i, c) in path.chars().enumerate() {
        if matches!(c, '\\' | '*' | '?' | '[') || (i == 0 && c == '#') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    Some(pattern)
}

fn is_live(target: &DiffTarget) -> bool {
    matches!(
        target,
        DiffTarget::Staged { .. }
            | DiffTarget::Unstaged { .. }
            | DiffTarget::Conflicted { .. }
            | DiffTarget::Worktree { .. }
            | DiffTarget::Untracked { .. }
    )
}

/// Identity of a working-tree diff: requests that differ only in context, whitespace or size limit
/// supersede each other too (the viewer toggled a setting).
#[derive(PartialEq)]
struct LiveKey {
    repo: PathBuf,
    target: DiffTarget,
    path: String,
    old_path: Option<String>,
}

/// Cancel flag of the newest running request per working-tree diff (at most one entry per key).
static LIVE: Mutex<Vec<(LiveKey, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// A registered working-tree diff run; unregisters on drop.
struct LiveRun {
    cancel: Arc<AtomicBool>,
}

impl LiveRun {
    /// Register a run for `req`, cancelling the previous run of the same diff if it is still going. Keyed
    /// per file rather than per repository, so two working-tree diffs shown at once never cancel each other.
    fn begin(repo: &Path, req: &DiffRequest) -> LiveRun {
        let key = LiveKey {
            repo: repo.to_path_buf(),
            target: req.target.clone(),
            path: req.path.clone(),
            old_path: req.old_path.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        match live.iter_mut().find(|(k, _)| *k == key) {
            Some((_, flag)) => std::mem::replace(flag, cancel.clone()).store(true, Ordering::Relaxed),
            None => live.push((key, cancel.clone())),
        }
        LiveRun { cancel }
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, flag)| !Arc::ptr_eq(flag, &self.cancel));
    }
}

/// Receives a patch from git chunk by chunk. It is buffered while it fits in `max` bytes; once it
/// outgrows that it is `too_large`: the buffer is dropped and the whole patch is parsed on the fly for
/// headers and line counts only, so memory stays near `max` whatever the patch size and the `+a −d`
/// counts stay exact.
struct PatchSink {
    max: u64,
    /// Bytes received so far.
    len: u64,
    buf: Vec<u8>,
    /// Headers-and-counts parse, from the moment the patch outgrew `max`.
    over: Option<LineFeed>,
}

impl PatchSink {
    fn new(max: u64) -> Self {
        PatchSink { max, len: 0, buf: Vec::new(), over: None }
    }

    /// Take the next chunk; false once the rest of the patch cannot change the result (git may stop).
    fn push(&mut self, chunk: &[u8]) -> bool {
        self.len = self.len.saturating_add(chunk.len() as u64);
        if self.over.is_none() && self.len <= self.max {
            self.buf.extend_from_slice(chunk);
            return true;
        }
        let buf = &mut self.buf;
        let feed = self.over.get_or_insert_with(|| {
            let mut feed = LineFeed::new(Parser::new(false));
            feed.feed(&std::mem::take(buf));
            feed
        });
        feed.feed(chunk);
        !feed.settled()
    }

    fn finish(self) -> FileDiff {
        match self.over {
            Some(feed) => FileDiff { too_large: true, ..feed.finish() },
            None => parse_unified(&self.buf),
        }
    }
}

/// Untracked file vs nothing (`git diff --no-index -- /dev/null <path>`).
fn untracked_diff(
    cwd: &Path,
    path: &str,
    flags: &[String],
    max: u64,
    cancel: Option<Arc<AtomicBool>>,
) -> AppResult<FileDiff> {
    let full = cwd.join(relative_path(path)?);
    let meta = std::fs::symlink_metadata(&full).ok();
    if path.ends_with('/') || meta.as_ref().is_some_and(|m| m.is_dir()) {
        return Ok(FileDiff { header_lines: vec!["nested repository".to_string()], ..Default::default() });
    }
    // FIFOs, sockets and devices are never listed as untracked; `diff --no-index` would block reading a FIFO.
    if meta.as_ref().is_some_and(|m| !m.is_file() && !m.file_type().is_symlink()) {
        return Err(AppError::msg(format!("{path} is not a regular file")));
    }
    // A text file larger than the cap cannot produce a smaller patch: skip running git on it.
    if let Some(m) = meta.as_ref().filter(|m| m.is_file() && m.len() > max) {
        if !starts_with_nul(&full) {
            let mode = if is_executable(m) { "100755" } else { "100644" };
            return Ok(FileDiff {
                too_large: true,
                new_mode: Some(mode.to_string()),
                header_lines: vec![format!("new file mode {mode}")],
                ..Default::default()
            });
        }
    }
    let mut git = Git::new(cwd)
        .args(["diff", "--no-index"])
        .args(flags)
        .args(["-O/dev/null", "--", "/dev/null", path])
        .literal_paths();
    if let Some(flag) = cancel {
        git = git.cancel_on(flag);
    }
    let mut patch = PatchSink::new(max);
    let mut stopped = false;
    let out = git.output_streaming(|chunk| {
        let more = patch.push(chunk);
        stopped = !more;
        more
    })?;
    // Exit 1 = files differ, but also "error: Could not access …" (then stdout is empty).
    let failed = !stopped && out.code != 0 && (out.code != 1 || (patch.len == 0 && !out.stderr.is_empty()));
    if failed {
        return Err(AppError::Git {
            args: format!("diff --no-index -- /dev/null {path}"),
            code: Some(out.code),
            stderr: lossy(&out.stderr),
        });
    }
    Ok(patch.finish())
}

/// NUL byte in the first 8000 bytes (unreadable files count as text).
fn starts_with_nul(path: &Path) -> bool {
    let mut head = Vec::new();
    match std::fs::File::open(path) {
        Ok(f) => f.take(BINARY_SNIFF).read_to_end(&mut head).is_ok() && memchr::memchr(0, &head).is_some(),
        Err(_) => false,
    }
}

/// Owner execute bit, which is what git records as `100755`.
fn is_executable(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o100 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before the first `diff ` line.
    Preamble,
    /// Extended header lines of a file section.
    Header,
    /// Inside a hunk whose line counts are not exhausted yet.
    Body,
    /// After a hunk: another hunk, a trailing `\ No newline` marker or the next section may follow.
    Between,
    /// Binary patch data, ignored up to the next section.
    Skip,
}

/// Feeds a byte stream to a `Parser` line by line; a line may span chunks.
struct LineFeed {
    parser: Parser,
    /// Start of a line whose end has not arrived yet.
    partial: Vec<u8>,
    /// Longest line prefix handed to the parser.
    cap: usize,
    /// The parser needs no more input (its file section ended, or the counts are settled).
    ended: bool,
}

impl LineFeed {
    fn new(parser: Parser) -> Self {
        // Counting needs only the first bytes of a line, so a single huge line cannot pile up in
        // `partial`; a parse that keeps the lines needs them whole.
        let cap = if parser.keep_lines { usize::MAX } else { COUNT_LINE_CAP };
        LineFeed { parser, partial: Vec::new(), cap, ended: false }
    }

    fn feed(&mut self, mut bytes: &[u8]) {
        while !self.ended {
            let Some(nl) = memchr::memchr(b'\n', bytes) else {
                let room = self.cap.saturating_sub(self.partial.len());
                self.partial.extend_from_slice(&bytes[..bytes.len().min(room)]);
                return;
            };
            let line = &bytes[..nl];
            if self.partial.is_empty() {
                self.ended = !self.parser.line(&line[..line.len().min(self.cap)]);
            } else {
                let room = self.cap.saturating_sub(self.partial.len());
                self.partial.extend_from_slice(&line[..line.len().min(room)]);
                let whole = std::mem::take(&mut self.partial);
                self.ended = !self.parser.line(&whole);
                // Keep the allocation for the next split line.
                self.partial = whole;
                self.partial.clear();
            }
            bytes = &bytes[nl + 1..];
        }
    }

    /// True once the rest of the input cannot change the result (see `Parser::settle`); later input is
    /// ignored.
    fn settled(&mut self) -> bool {
        if !self.ended && self.parser.settle() {
            self.ended = true;
        }
        self.ended
    }

    /// The result, after the last line (which may lack its `\n`).
    fn finish(mut self) -> FileDiff {
        if !self.ended && !self.partial.is_empty() {
            let last = std::mem::take(&mut self.partial);
            self.parser.line(&last);
        }
        self.parser.diff
    }
}

/// Line-by-line state machine behind `parse_unified`.
struct Parser {
    /// Build hunks and lines; false = headers and counts only (oversized diffs).
    keep_lines: bool,
    diff: FileDiff,
    state: State,
    /// `diff ` line of the first section.
    first: Vec<u8>,
    /// The first section deletes the file (start of a type change).
    first_deletes: bool,
    /// The current section creates the file (`new file mode`): one hunk of additions only.
    creates: bool,
    sections: u8,
    /// Lines still expected in the current hunk, per parent (1 for plain diffs) and in the result.
    old_left: Vec<u32>,
    new_left: u32,
    old_no: u32,
    new_no: u32,
    /// The previous line completed a hunk, so a `\ No newline` marker still belongs to it.
    marker_ok: bool,
}

struct HunkHeader {
    /// `(start, count)` per parent.
    old: Vec<(u32, u32)>,
    new: (u32, u32),
}

impl Parser {
    fn new(keep_lines: bool) -> Self {
        Parser {
            keep_lines,
            diff: FileDiff::default(),
            state: State::Preamble,
            first: Vec::new(),
            first_deletes: false,
            creates: false,
            sections: 0,
            old_left: Vec::new(),
            new_left: 0,
            old_no: 0,
            new_no: 0,
            marker_ok: false,
        }
    }

    fn parse(self, buf: &[u8]) -> FileDiff {
        let mut feed = LineFeed::new(self);
        feed.feed(buf);
        feed.finish()
    }

    /// Handle one line (without its `\n`); false = the file section is over.
    fn line(&mut self, line: &[u8]) -> bool {
        match self.state {
            State::Preamble => {
                if [&b"diff --git "[..], b"diff --cc ", b"diff --combined "].iter().any(|p| line.starts_with(p)) {
                    self.first = line.to_vec();
                    self.sections = 1;
                    self.state = State::Header;
                }
                true
            }
            State::Body => {
                if self.body_line(line) {
                    return true;
                }
                // Not a hunk line although lines were still expected: resynchronize.
                self.state = State::Between;
                self.marker_ok = false;
                self.line(line)
            }
            _ if line.starts_with(b"diff ") => self.next_section(line),
            State::Header => {
                self.header_line(line);
                true
            }
            State::Between => {
                if line.first() == Some(&b'\\') && self.marker_ok {
                    self.push(3, None, None, b"");
                } else if line.starts_with(b"@@") {
                    self.start_hunk(line);
                }
                self.marker_ok = false;
                true
            }
            State::Skip => true,
        }
    }

    /// A second section continues the diff only for a type change (deletion followed by creation).
    fn next_section(&mut self, line: &[u8]) -> bool {
        if self.sections == 1 && self.first_deletes && line == self.first.as_slice() {
            self.sections = 2;
            self.state = State::Header;
            self.creates = false;
            self.marker_ok = false;
            return true;
        }
        false
    }

    /// Counts-only parsing inside the hunk of a section that creates the file (`@@ -0,0 +1,N @@`, also
    /// a type change's second section): every line still expected is an addition, and nothing after the
    /// hunk counts (a creation has a single hunk and never continues into another section). Count them
    /// now and return true: the result is final, so the rest of the patch may go unread. Never for a
    /// `-U0` insertion at the top of an existing file, which looks the same but lacks `new file mode`.
    fn settle(&mut self) -> bool {
        if self.keep_lines || !self.creates || self.state != State::Body || self.old_left.iter().any(|n| *n > 0) {
            return false;
        }
        self.diff.additions = self.diff.additions.saturating_add(self.new_left);
        self.new_left = 0;
        self.state = State::Between;
        true
    }

    fn header_line(&mut self, line: &[u8]) {
        if line.starts_with(b"@@") {
            self.start_hunk(line);
            return;
        }
        if line.starts_with(b"--- ") || line.starts_with(b"+++ ") {
            return;
        }
        if line.starts_with(b"Binary files ") {
            self.diff.binary = true;
            return;
        }
        if line == b"GIT binary patch" {
            self.diff.binary = true;
            self.state = State::Skip;
            return;
        }
        let text = lossy(line);
        let d = &mut self.diff;
        if let Some(m) = text.strip_prefix("old mode ") {
            d.old_mode = Some(m.to_string());
        } else if let Some(m) = text.strip_prefix("new mode ") {
            d.new_mode = Some(m.to_string());
        } else if let Some(m) = text.strip_prefix("deleted file mode ") {
            d.old_mode = Some(first_parent(m).to_string());
            if self.sections == 1 {
                self.first_deletes = true;
            }
        } else if let Some(m) = text.strip_prefix("new file mode ") {
            d.new_mode = Some(m.to_string());
            self.creates = true;
        } else if let Some((old, new)) = text.strip_prefix("mode ").and_then(|m| m.split_once("..")) {
            // Combined diff: `mode <p1>,<p2>..<result>`.
            d.old_mode = Some(first_parent(old).to_string());
            d.new_mode = Some(new.to_string());
        } else if let Some((old, new)) =
            text.strip_prefix("index ").and_then(|ids| ids.split(' ').next()).and_then(|ids| ids.split_once(".."))
        {
            set_oid(&mut d.old_oid, first_parent(old));
            set_oid(&mut d.new_oid, new);
        }
        d.header_lines.push(text);
    }

    fn start_hunk(&mut self, line: &[u8]) {
        self.marker_ok = false;
        let Some(h) = parse_hunk_header(line) else {
            self.state = State::Between;
            return;
        };
        let (old_start, old_lines) = h.old[0];
        self.old_left = h.old.iter().map(|r| r.1).collect();
        self.new_left = h.new.1;
        self.old_no = old_start;
        self.new_no = h.new.0;
        if self.keep_lines {
            self.diff.hunks.push(Hunk {
                header: lossy(line),
                old_start,
                old_lines,
                new_start: h.new.0,
                new_lines: h.new.1,
                lines: Vec::new(),
            });
        }
        self.state = if self.hunk_done() { State::Between } else { State::Body };
    }

    /// Consume one hunk line; false when the line cannot be part of a hunk.
    fn body_line(&mut self, line: &[u8]) -> bool {
        if line.first() == Some(&b'\\') {
            self.push(3, None, None, b"");
            return true;
        }
        // One prefix column per parent; an empty line is an empty context line.
        let cols = &line[..self.old_left.len().min(line.len())];
        if !cols.iter().all(|c| matches!(c, b' ' | b'+' | b'-')) {
            return false;
        }
        let text = &line[cols.len()..];
        let removed = cols.contains(&b'-');
        for (k, left) in self.old_left.iter_mut().enumerate() {
            let c = cols.get(k).copied().unwrap_or(b' ');
            if c == (if removed { b'-' } else { b' ' }) {
                *left = left.saturating_sub(1);
            }
        }
        if !removed {
            self.new_left = self.new_left.saturating_sub(1);
        }
        match (removed, cols.first().copied().unwrap_or(b' ')) {
            (true, b'-') => {
                let no = self.old_no;
                self.old_no = no.saturating_add(1);
                self.diff.deletions = self.diff.deletions.saturating_add(1);
                self.push(2, Some(no), None, text);
            }
            // Removed from the result but absent from the first parent: not part of this projection.
            (true, _) => {}
            (false, b'+') => {
                let no = self.new_no;
                self.new_no = no.saturating_add(1);
                self.diff.additions = self.diff.additions.saturating_add(1);
                self.push(1, None, Some(no), text);
            }
            (false, _) => {
                let (old, new) = (self.old_no, self.new_no);
                self.old_no = old.saturating_add(1);
                self.new_no = new.saturating_add(1);
                self.push(0, Some(old), Some(new), text);
            }
        }
        if self.hunk_done() {
            self.state = State::Between;
            self.marker_ok = true;
        }
        true
    }

    fn hunk_done(&self) -> bool {
        self.new_left == 0 && self.old_left.iter().all(|n| *n == 0)
    }

    fn push(&mut self, kind: u8, old: Option<u32>, new: Option<u32>, text: &[u8]) {
        if !self.keep_lines {
            return;
        }
        if let Some(h) = self.diff.hunks.last_mut() {
            h.lines.push(DiffLine(kind, old, new, lossy(text)));
        }
    }
}

/// `@@ -a[,b] +c[,d] @@[ heading]`, or the combined form with one `-` range per parent and one more `@`.
fn parse_hunk_header(line: &[u8]) -> Option<HunkHeader> {
    let ats = line.iter().take_while(|b| **b == b'@').count();
    if ats < 2 {
        return None;
    }
    let mut rest = &line[ats..];
    let mut old = Vec::with_capacity(ats - 1);
    for _ in 1..ats {
        let (range, tail) = parse_range(rest.strip_prefix(b" -")?)?;
        old.push(range);
        rest = tail;
    }
    let (new, tail) = parse_range(rest.strip_prefix(b" +")?)?;
    let tail = tail.strip_prefix(b" ")?;
    (tail.len() >= ats && tail[..ats].iter().all(|b| *b == b'@')).then_some(HunkHeader { old, new })
}

/// `start[,count]` (missing count = 1).
fn parse_range(s: &[u8]) -> Option<((u32, u32), &[u8])> {
    let (start, rest) = parse_num(s)?;
    match rest.strip_prefix(b",") {
        Some(r) => {
            let (count, rest) = parse_num(r)?;
            Some(((start, count), rest))
        }
        None => Some(((start, 1), rest)),
    }
}

fn parse_num(s: &[u8]) -> Option<(u32, &[u8])> {
    let n = s.iter().take_while(|b| b.is_ascii_digit()).count();
    let v = std::str::from_utf8(&s[..n]).ok()?.parse().ok()?;
    Some((v, &s[n..]))
}

/// First entry of a combined-diff list (`a,b` → `a`).
fn first_parent(s: &str) -> &str {
    s.split(',').next().unwrap_or(s)
}

/// Keep the first real blob id seen for a side (the all-zero id means "no blob").
fn set_oid(slot: &mut Option<String>, oid: &str) {
    if slot.is_none() && oid.bytes().any(|b| b != b'0') {
        *slot = Some(oid.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(lines: &[DiffLine]) -> Vec<(u8, Option<u32>, Option<u32>, &str)> {
        lines.iter().map(|l| (l.0, l.1, l.2, l.3.as_str())).collect()
    }

    const DIFF_RENAME: &[u8] = b"diff --git a/long.txt b/renamed long.txt\n\
        similarity index 96%\n\
        rename from long.txt\n\
        rename to renamed long.txt\n\
        index 35be908f97592ab48b79b15942eba5aa7959aab9..afdb0f7f895ef388c1c50e58a8ac4e73bc561458 100644\n\
        --- a/long.txt\n\
        +++ b/renamed long.txt\t\n\
        @@ -17,7 +17,7 @@ long line 16\n\
        \x20long line 17\n\
        \x20long line 18\n\
        \x20long line 19\n\
        -long line 20\n\
        +long line twenty\n\
        \x20long line 21\n\
        \x20long line 22\n\
        \x20long line 23\n";
    const DIFF_BINARY: &[u8] = b"diff --git a/bin.dat b/bin.dat\n\
        index 83536cff1521d812a0feee584bd568ea709389f8..8e498a275ff4d37b96e6a6496e6ef26af35109d4 100644\n\
        Binary files a/bin.dat and b/bin.dat differ\n";
    const DIFF_MODE: &[u8] = b"diff --git a/script.sh b/script.sh\n\
        old mode 100644\n\
        new mode 100755\n";
    const DIFF_DELETED: &[u8] = b"diff --git a/README.md b/README.md\n\
        deleted file mode 100644\n\
        index 8178c76d627cade75005b40711b92f4177bc6cfc..0000000000000000000000000000000000000000\n\
        --- a/README.md\n\
        +++ /dev/null\n\
        @@ -1 +0,0 @@\n\
        -readme\n";
    const DIFF_EMPTY_NEW: &[u8] = b"diff --git a/empty2.txt b/empty2.txt\n\
        new file mode 100644\n\
        index 0000000000000000000000000000000000000000..e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\n";
    const DIFF_NOEOL_BOTH: &[u8] = b"diff --git a/nonl.txt b/nonl.txt\n\
        index daf3c426ac9136633240049e259e1e3f6d7dc26c..f9f7b69167a2970e1eecc6f2bc11cb37e9c715c1 100644\n\
        --- a/nonl.txt\n\
        +++ b/nonl.txt\n\
        @@ -1,2 +1,2 @@\n\
        \x20keep\n\
        -last no newline\n\
        \\ No newline at end of file\n\
        +last no newline changed\n\
        \\ No newline at end of file\n";
    const DIFF_NOEOL_OLD: &[u8] = b"diff --git a/nonl-old.txt b/nonl-old.txt\n\
        index 31cab939b11e973f3b3a88905fe7c5066c5dc46d..11f660feae9dd8551c364f0da9bac0483e94099c 100644\n\
        --- a/nonl-old.txt\n\
        +++ b/nonl-old.txt\n\
        @@ -1,2 +1,2 @@\n\
        \x20x\n\
        -old-without-eol\n\
        \\ No newline at end of file\n\
        +old-without-eol\n";
    const DIFF_NOEOL_NEW: &[u8] = b"diff --git a/nonl-new.txt b/nonl-new.txt\n\
        index 96bda30e0c2cef9a2f01fe00388a3c42180ba71a..5d9b138163b966315b60458d835cfbb3b3d77b63 100644\n\
        --- a/nonl-new.txt\n\
        +++ b/nonl-new.txt\n\
        @@ -1,2 +1,2 @@\n\
        \x20y\n\
        -new-without-eol\n\
        +new-without-eol\n\
        \\ No newline at end of file\n";
    const DIFF_CRLF: &[u8] = b"diff --git a/crlf.txt b/crlf.txt\n\
        index 75f18be4fd2a229531097e94374316da64baa0f2..4b63a83b9df0e310d96c3ce18f108358874aa0a8 100644\n\
        --- a/crlf.txt\n\
        +++ b/crlf.txt\n\
        @@ -1,3 +1,3 @@\n\
        \x20crlf1\r\n\
        -crlf2\r\n\
        +crlf2 changed\r\n\
        \x20crlf3\r\n";
    const DIFF_DASHES: &[u8] = b"diff --git a/dashes.txt b/dashes.txt\n\
        index 06cda4b7b93b86b74b1f1fc9b676a8f7936bc736..9880d0735333103ce1a0c913e1560cfd219f106f 100644\n\
        --- a/dashes.txt\n\
        +++ b/dashes.txt\n\
        @@ -1,4 +1,4 @@\n\
        \x20keep1\n\
        --- dash comment\n\
        \x20++ plus plus\n\
        +-- dash new\n\
        \x20keep2\n";
    const DIFF_UNICODE: &[u8] =
        b"diff --git a/\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt b/\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\n\
        index 7a44627979a55254e1f25e2bdfcfd8b269868610..d6bbe256e5c8d78a973aeff07d955365b7ffb5b4 100644\n\
        --- a/\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\t\n\
        +++ b/\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\t\n\
        @@ -1 +1 @@\n\
        -\xed\x95\x9c\xea\xb8\x80\n\
        +\xed\x95\x9c\xea\xb8\x80 \xec\x88\x98\xec\xa0\x95\n";
    const DIFF_TYPECHANGE: &[u8] = b"diff --git a/link b/link\n\
        deleted file mode 120000\n\
        index 8d14cbf983b3fad683171c9418998d9f68340823..0000000000000000000000000000000000000000\n\
        --- a/link\n\
        +++ /dev/null\n\
        @@ -1 +0,0 @@\n\
        -a.txt\n\
        \\ No newline at end of file\n\
        diff --git a/link b/link\n\
        new file mode 100644\n\
        index 0000000000000000000000000000000000000000..3f899ea7ab51da801dbacbf633c168b0591d7765\n\
        --- /dev/null\n\
        +++ b/link\n\
        @@ -0,0 +1 @@\n\
        +now a file\n";
    const DIFF_COPY: &[u8] = b"diff --git a/copy-src.txt b/copy-dst.txt\n\
        similarity index 100%\n\
        copy from copy-src.txt\n\
        copy to copy-dst.txt\n\
        diff --git a/copy-src.txt b/copy-src.txt\n\
        index 627165f5e27b6486daf3678a4edf4688de9aa47c..6774ffc32a1dfda3bb15bdc01cd082d3e6c79e3f 100644\n\
        --- a/copy-src.txt\n\
        +++ b/copy-src.txt\n\
        @@ -28,3 +28,4 @@ copy source 27\n\
        \x20copy source 28\n\
        \x20copy source 29\n\
        \x20copy source 30\n\
        +copy source extra\n";
    const DIFF_ROOT_WITH_ID: &[u8] = b"0205c3f1b4884b20e95e8688230b654e76cd0067\n\
        diff --git a/a.txt b/a.txt\n\
        new file mode 100644\n\
        index 0000000000000000000000000000000000000000..83db48f84ec878fbfb30b46d16630e944e34f205\n\
        --- /dev/null\n\
        +++ b/a.txt\n\
        @@ -0,0 +1,3 @@\n\
        +line1\n\
        +line2\n\
        +line3\n";
    const DIFF_SUBMODULE_ADD: &[u8] = b"diff --git a/libs/sub b/libs/sub\n\
        new file mode 160000\n\
        index 0000000000000000000000000000000000000000..2d283f156df461aa10b92870abe12178e787e4fe\n\
        --- /dev/null\n\
        +++ b/libs/sub\n\
        @@ -0,0 +1 @@\n\
        +Subproject commit 2d283f156df461aa10b92870abe12178e787e4fe\n";
    const DIFF_SUBMODULE_BUMP: &[u8] = b"diff --git a/libs/sub b/libs/sub\n\
        index 2d283f156df461aa10b92870abe12178e787e4fe..6616a9a889347a2147e684f6f4d365b33259109a 160000\n\
        --- a/libs/sub\n\
        +++ b/libs/sub\n\
        @@ -1 +1 @@\n\
        -Subproject commit 2d283f156df461aa10b92870abe12178e787e4fe\n\
        +Subproject commit 6616a9a889347a2147e684f6f4d365b33259109a\n";
    const DIFF_MERGE_FIRST_PARENT: &[u8] = b"diff --git a/a.txt b/a.txt\n\
        index 4a1223fb7d818b89cbe1322552e38c207295f9ea..df814d73085d7949feb11a96fb28593257959f31 100644\n\
        --- a/a.txt\n\
        +++ b/a.txt\n\
        @@ -1,4 +1,4 @@\n\
        \x20line0\n\
        \x20line1\n\
        -line2\n\
        +line2 side\n\
        \x20line3\n";
    const DIFF_STAGED_RENAME: &[u8] = b"diff --git a/side.txt b/side-renamed.txt\n\
        similarity index 100%\n\
        rename from side.txt\n\
        rename to side-renamed.txt\n";
    const DIFF_UNSTAGED: &[u8] = b"diff --git a/a.txt b/a.txt\n\
        index 96aabf3ef4cbee8f175966e4f2f23e8fe2240045..07131906083cf79c9937a5df386cc9ecfd6a93ec 100644\n\
        --- a/a.txt\n\
        +++ b/a.txt\n\
        @@ -3,3 +3,4 @@ line1\n\
        \x20line2 side\n\
        \x20line3\n\
        \x20local staged\n\
        +local unstaged\n";
    const DIFF_UNSTAGED_DELETED_EMPTY: &[u8] = b"diff --git a/empty.txt b/empty.txt\n\
        deleted file mode 100644\n\
        index e69de29bb2d1d6434b8b29ae775ad8c2e48c5391..0000000000000000000000000000000000000000\n";
    const DIFF_SUBMODULE_DIRTY: &[u8] = b"diff --git a/libs/sub b/libs/sub\n\
        --- a/libs/sub\n\
        +++ b/libs/sub\n\
        @@ -1 +1 @@\n\
        -Subproject commit 6616a9a889347a2147e684f6f4d365b33259109a\n\
        +Subproject commit 6616a9a889347a2147e684f6f4d365b33259109a-dirty\n";
    const DIFF_UNTRACKED_UNICODE: &[u8] =
        b"diff --git a/\xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc.txt b/\xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc.txt\n\
        new file mode 100644\n\
        index 0000000000000000000000000000000000000000..47b0484ebb45a75484bf6e743fa589c855e4ff55\n\
        --- /dev/null\n\
        +++ b/\xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc.txt\t\n\
        @@ -0,0 +1 @@\n\
        +\xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc\n";
    const DIFF_UNTRACKED_BINARY: &[u8] = b"diff --git a/untracked.bin b/untracked.bin\n\
        new file mode 100644\n\
        index 0000000000000000000000000000000000000000..87ae6b695deceaf160611414f7dcd5c7366b2e79\n\
        Binary files /dev/null and b/untracked.bin differ\n";
    const DIFF_UNTRACKED_SYMLINK: &[u8] = b"diff --git a/ulink b/ulink\n\
        new file mode 120000\n\
        index 0000000000000000000000000000000000000000..fe626378a1e4b55b826558a9870bc319c7e8289f\n\
        --- /dev/null\n\
        +++ b/ulink\n\
        @@ -0,0 +1 @@\n\
        +\xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc.txt\n\
        \\ No newline at end of file\n";
    const DIFF_CONFLICT_OURS: &[u8] = b"* Unmerged path a.txt\n\
        diff --git a/a.txt b/a.txt\n\
        index e6a1095499ba19c2c8db87a41478ef7dad492859..060bea33ee81e3cca15b1f20ba97c2e39f93f25f 100644\n\
        --- a/a.txt\n\
        +++ b/a.txt\n\
        @@ -1,4 +1,8 @@\n\
        \x20line0\n\
        \x20line1\n\
        +<<<<<<< HEAD\n\
        \x20OURS\n\
        +=======\n\
        +THEIRS\n\
        +>>>>>>> theirs\n\
        \x20line3\n";
    const DIFF_COMBINED: &[u8] =
        b"diff --cc a.txt\n\
        index e6a1095499ba19c2c8db87a41478ef7dad492859,0c5e4de53b5b15c144fcc147c7140b381f49090c..0000000000000000000000000000000000000000\n\
        --- a/a.txt\n\
        +++ b/a.txt\n\
        @@@ -1,4 -1,4 +1,8 @@@\n\
        \x20 line0\n\
        \x20 line1\n\
        ++<<<<<<< HEAD\n\
        \x20+OURS\n\
        ++=======\n\
        + THEIRS\n\
        ++>>>>>>> theirs\n\
        \x20 line3\n";
    const DIFF_COMBINED_EVIL: &[u8] =
        b"6bf7769671287552ce91154103e66489e9417834\n\
        diff --combined f.txt\n\
        index a7bc997ebe8cf84988b83d2e83f1d193124fe593,54cb2ce97b66e7aa831a55c3f9cb0e16bbaaef4b..768c98ca25d9db9b3528aab8f0d120816af31f16\n\
        --- a/f.txt\n\
        +++ b/f.txt\n\
        @@@ -1,4 -1,4 +1,5 @@@\n\
        \x20 a\n\
        - B\n\
        - c\n\
        \x20-b\n\
        ++B2\n\
        + C\n\
        \x20 d\n\
        ++added\n";
    const DIFF_U0: &[u8] = b"diff --git a/f.txt b/f.txt\n\
        index eaabd89809cd1c5394765293efc9bbf3e9d2051c..677ccf6b8f2712a64787b650d75ce3c7a0917562 100644\n\
        --- a/f.txt\n\
        +++ b/f.txt\n\
        @@ -2 +2 @@ a  b\n\
        -c\n\
        +c changed\n";
    const DIFF_WS_MODE: &[u8] = b"diff --git a/m.txt b/m.txt\n\
        old mode 100644\n\
        new mode 100755\n\
        index 587be6b4c3f93f93c489c0111bba5596147a26cb..9f009d654dcda3ad51bc3d66f8636a2d0c22c64a\n";

    #[test]
    fn rename_with_edits() {
        let d = parse_unified(DIFF_RENAME);
        assert_eq!(
            d.header_lines,
            [
                "similarity index 96%",
                "rename from long.txt",
                "rename to renamed long.txt",
                "index 35be908f97592ab48b79b15942eba5aa7959aab9..afdb0f7f895ef388c1c50e58a8ac4e73bc561458 100644",
            ]
        );
        assert_eq!(d.old_oid.as_deref(), Some("35be908f97592ab48b79b15942eba5aa7959aab9"));
        assert_eq!(d.new_oid.as_deref(), Some("afdb0f7f895ef388c1c50e58a8ac4e73bc561458"));
        assert_eq!((d.old_mode.as_ref(), d.new_mode.as_ref(), d.binary), (None, None, false));
        assert_eq!(d.hunks.len(), 1);
        let h = &d.hunks[0];
        assert_eq!(h.header, "@@ -17,7 +17,7 @@ long line 16");
        assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (17, 7, 17, 7));
        assert_eq!(
            summary(&h.lines),
            [
                (0, Some(17), Some(17), "long line 17"),
                (0, Some(18), Some(18), "long line 18"),
                (0, Some(19), Some(19), "long line 19"),
                (2, Some(20), None, "long line 20"),
                (1, None, Some(20), "long line twenty"),
                (0, Some(21), Some(21), "long line 21"),
                (0, Some(22), Some(22), "long line 22"),
                (0, Some(23), Some(23), "long line 23"),
            ]
        );
        assert_eq!((d.additions, d.deletions), (1, 1));
    }

    #[test]
    fn headers_without_hunks() {
        let d = parse_unified(DIFF_BINARY);
        assert!(d.binary && d.hunks.is_empty());
        assert_eq!(d.header_lines.len(), 1);
        assert_eq!(d.new_oid.as_deref(), Some("8e498a275ff4d37b96e6a6496e6ef26af35109d4"));

        let d = parse_unified(DIFF_MODE);
        assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (Some("100644"), Some("100755")));
        assert_eq!(d.header_lines, ["old mode 100644", "new mode 100755"]);
        assert!(d.hunks.is_empty() && d.old_oid.is_none());

        let d = parse_unified(DIFF_EMPTY_NEW);
        assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (None, Some("100644")));
        assert_eq!(
            (d.old_oid.as_deref(), d.new_oid.as_deref()),
            (None, Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"))
        );
        assert!(d.hunks.is_empty() && !d.binary);

        let d = parse_unified(DIFF_UNSTAGED_DELETED_EMPTY);
        assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (Some("100644"), None));
        assert_eq!(
            (d.old_oid.as_deref(), d.new_oid.as_deref()),
            (Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"), None)
        );
        assert!(d.hunks.is_empty());

        let d = parse_unified(DIFF_STAGED_RENAME);
        assert_eq!(d.header_lines, ["similarity index 100%", "rename from side.txt", "rename to side-renamed.txt"]);
        assert!(d.hunks.is_empty() && d.old_oid.is_none());

        let d = parse_unified(DIFF_WS_MODE);
        assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (Some("100644"), Some("100755")));
        assert_eq!(d.new_oid.as_deref(), Some("9f009d654dcda3ad51bc3d66f8636a2d0c22c64a"));
        assert!(d.hunks.is_empty());

        let d = parse_unified(b"");
        assert!(d.header_lines.is_empty() && d.hunks.is_empty() && !d.binary);
    }

    #[test]
    fn deleted_file() {
        let d = parse_unified(DIFF_DELETED);
        assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (Some("100644"), None));
        assert_eq!(
            (d.old_oid.as_deref(), d.new_oid.as_deref()),
            (Some("8178c76d627cade75005b40711b92f4177bc6cfc"), None)
        );
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(
            (d.hunks[0].old_start, d.hunks[0].old_lines, d.hunks[0].new_start, d.hunks[0].new_lines),
            (1, 1, 0, 0)
        );
        assert_eq!(summary(&d.hunks[0].lines), [(2, Some(1), None, "readme")]);
        assert_eq!((d.additions, d.deletions), (0, 1));
    }

    #[test]
    fn missing_newline_markers() {
        let d = parse_unified(DIFF_NOEOL_BOTH);
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "keep"),
                (2, Some(2), None, "last no newline"),
                (3, None, None, ""),
                (1, None, Some(2), "last no newline changed"),
                (3, None, None, ""),
            ]
        );
        let d = parse_unified(DIFF_NOEOL_OLD);
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "x"),
                (2, Some(2), None, "old-without-eol"),
                (3, None, None, ""),
                (1, None, Some(2), "old-without-eol")
            ]
        );
        let d = parse_unified(DIFF_NOEOL_NEW);
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "y"),
                (2, Some(2), None, "new-without-eol"),
                (1, None, Some(2), "new-without-eol"),
                (3, None, None, "")
            ]
        );
        let d = parse_unified(DIFF_UNTRACKED_SYMLINK);
        assert_eq!(d.new_mode.as_deref(), Some("120000"));
        assert_eq!(summary(&d.hunks[0].lines), [(1, None, Some(1), "새 파일.txt"), (3, None, None, "")]);
    }

    #[test]
    fn crlf_dashes_and_unicode() {
        let d = parse_unified(DIFF_CRLF);
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "crlf1\r"),
                (2, Some(2), None, "crlf2\r"),
                (1, None, Some(2), "crlf2 changed\r"),
                (0, Some(3), Some(3), "crlf3\r"),
            ]
        );
        let d = parse_unified(DIFF_DASHES);
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "keep1"),
                (2, Some(2), None, "-- dash comment"),
                (0, Some(3), Some(2), "++ plus plus"),
                (1, None, Some(3), "-- dash new"),
                (0, Some(4), Some(4), "keep2"),
            ]
        );
        assert_eq!((d.additions, d.deletions), (1, 1));
        let d = parse_unified(DIFF_UNICODE);
        assert_eq!(summary(&d.hunks[0].lines), [(2, Some(1), None, "한글"), (1, None, Some(1), "한글 수정")]);
        let d = parse_unified(DIFF_UNTRACKED_UNICODE);
        assert_eq!((d.new_mode.as_deref(), d.old_oid.as_deref()), (Some("100644"), None));
        assert_eq!(summary(&d.hunks[0].lines), [(1, None, Some(1), "새 파일")]);
    }

    #[test]
    fn type_change_sections_are_merged() {
        let d = parse_unified(DIFF_TYPECHANGE);
        assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (Some("120000"), Some("100644")));
        assert_eq!(d.old_oid.as_deref(), Some("8d14cbf983b3fad683171c9418998d9f68340823"));
        assert_eq!(d.new_oid.as_deref(), Some("3f899ea7ab51da801dbacbf633c168b0591d7765"));
        assert_eq!(d.header_lines.len(), 4);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(summary(&d.hunks[0].lines), [(2, Some(1), None, "a.txt"), (3, None, None, "")]);
        assert_eq!(summary(&d.hunks[1].lines), [(1, None, Some(1), "now a file")]);
        assert_eq!((d.additions, d.deletions), (1, 1));
    }

    #[test]
    fn only_first_section_and_preamble_skipped() {
        let d = parse_unified(DIFF_COPY);
        assert_eq!(d.header_lines, ["similarity index 100%", "copy from copy-src.txt", "copy to copy-dst.txt"]);
        assert!(d.hunks.is_empty() && d.new_oid.is_none());

        let d = parse_unified(DIFF_ROOT_WITH_ID);
        assert_eq!(d.new_mode.as_deref(), Some("100644"));
        assert_eq!(
            summary(&d.hunks[0].lines),
            [(1, None, Some(1), "line1"), (1, None, Some(2), "line2"), (1, None, Some(3), "line3")]
        );

        let d = parse_unified(DIFF_CONFLICT_OURS);
        assert_eq!(d.old_oid.as_deref(), Some("e6a1095499ba19c2c8db87a41478ef7dad492859"));
        assert_eq!(d.hunks[0].lines.len(), 8);
        assert_eq!(d.hunks[0].lines[3], DiffLine(0, Some(3), Some(4), "OURS".into()));
        assert_eq!((d.additions, d.deletions), (4, 0));
    }

    #[test]
    fn submodules() {
        let d = parse_unified(DIFF_SUBMODULE_ADD);
        assert_eq!(d.new_mode.as_deref(), Some("160000"));
        assert_eq!(
            summary(&d.hunks[0].lines),
            [(1, None, Some(1), "Subproject commit 2d283f156df461aa10b92870abe12178e787e4fe")]
        );
        let d = parse_unified(DIFF_SUBMODULE_BUMP);
        assert_eq!(d.new_oid.as_deref(), Some("6616a9a889347a2147e684f6f4d365b33259109a"));
        assert_eq!(d.hunks[0].lines.len(), 2);
        let d = parse_unified(DIFF_SUBMODULE_DIRTY);
        assert!(d.old_oid.is_none() && d.header_lines.is_empty());
        assert_eq!(d.hunks[0].lines[1].3, "Subproject commit 6616a9a889347a2147e684f6f4d365b33259109a-dirty");
    }

    #[test]
    fn worktree_and_merge_hunks() {
        let d = parse_unified(DIFF_UNSTAGED);
        assert_eq!(d.hunks[0].header, "@@ -3,3 +3,4 @@ line1");
        assert_eq!(d.hunks[0].lines[3], DiffLine(1, None, Some(6), "local unstaged".into()));
        let d = parse_unified(DIFF_MERGE_FIRST_PARENT);
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "line0"),
                (0, Some(2), Some(2), "line1"),
                (2, Some(3), None, "line2"),
                (1, None, Some(3), "line2 side"),
                (0, Some(4), Some(4), "line3"),
            ]
        );
        let d = parse_unified(DIFF_U0);
        assert_eq!(
            (d.hunks[0].old_start, d.hunks[0].old_lines, d.hunks[0].new_start, d.hunks[0].new_lines),
            (2, 1, 2, 1)
        );
        assert_eq!(summary(&d.hunks[0].lines), [(2, Some(2), None, "c"), (1, None, Some(2), "c changed")]);
        let d = parse_unified(DIFF_UNTRACKED_BINARY);
        assert!(d.binary && d.hunks.is_empty());
        assert_eq!(d.new_mode.as_deref(), Some("100644"));
    }

    #[test]
    fn combined_diffs_project_onto_first_parent() {
        let d = parse_unified(DIFF_COMBINED);
        assert_eq!(d.old_oid.as_deref(), Some("e6a1095499ba19c2c8db87a41478ef7dad492859"));
        assert_eq!(d.new_oid, None);
        let h = &d.hunks[0];
        assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (1, 4, 1, 8));
        assert_eq!(
            summary(&h.lines),
            [
                (0, Some(1), Some(1), "line0"),
                (0, Some(2), Some(2), "line1"),
                (1, None, Some(3), "<<<<<<< HEAD"),
                (0, Some(3), Some(4), "OURS"),
                (1, None, Some(5), "======="),
                (1, None, Some(6), "THEIRS"),
                (1, None, Some(7), ">>>>>>> theirs"),
                (0, Some(4), Some(8), "line3"),
            ]
        );
        let d = parse_unified(DIFF_COMBINED_EVIL);
        assert_eq!(d.new_oid.as_deref(), Some("768c98ca25d9db9b3528aab8f0d120816af31f16"));
        assert_eq!(
            summary(&d.hunks[0].lines),
            [
                (0, Some(1), Some(1), "a"),
                (2, Some(2), None, "B"),
                (2, Some(3), None, "c"),
                (1, None, Some(2), "B2"),
                (1, None, Some(3), "C"),
                (0, Some(4), Some(4), "d"),
                (1, None, Some(5), "added"),
            ]
        );
        assert_eq!((d.additions, d.deletions), (3, 2));
    }

    #[test]
    fn oversized_diffs_keep_headers_and_counts() {
        let d = Parser::new(false).parse(DIFF_RENAME);
        assert!(d.hunks.is_empty());
        assert_eq!(d.header_lines.len(), 4);
        assert_eq!((d.additions, d.deletions), (1, 1));
        let d = Parser::new(false).parse(DIFF_TYPECHANGE);
        assert_eq!((d.new_mode.as_deref(), d.additions, d.deletions), (Some("100644"), 1, 1));
    }

    #[test]
    fn short_hunk_resynchronizes() {
        let buf =
            b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n-a\n+b\ndiff --git a/y b/y\n@@ -1 +1 @@\n-y\n";
        let d = parse_unified(buf);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(summary(&d.hunks[0].lines), [(2, Some(1), None, "a"), (1, None, Some(1), "b")]);
        let buf = b"diff --git a/x b/x\n@@ -1 +1 @@\n x\n\\ No newline at end of file\n@@ -9 +9 @@\n\n";
        let d = parse_unified(buf);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(summary(&d.hunks[0].lines), [(0, Some(1), Some(1), "x"), (3, None, None, "")]);
        assert_eq!(summary(&d.hunks[1].lines), [(0, Some(9), Some(9), "")]);
    }

    const FIXTURES: &[&[u8]] = &[
        DIFF_RENAME,
        DIFF_BINARY,
        DIFF_MODE,
        DIFF_DELETED,
        DIFF_EMPTY_NEW,
        DIFF_NOEOL_BOTH,
        DIFF_NOEOL_OLD,
        DIFF_NOEOL_NEW,
        DIFF_CRLF,
        DIFF_DASHES,
        DIFF_UNICODE,
        DIFF_TYPECHANGE,
        DIFF_COPY,
        DIFF_ROOT_WITH_ID,
        DIFF_SUBMODULE_ADD,
        DIFF_SUBMODULE_BUMP,
        DIFF_MERGE_FIRST_PARENT,
        DIFF_STAGED_RENAME,
        DIFF_UNSTAGED,
        DIFF_UNSTAGED_DELETED_EMPTY,
        DIFF_SUBMODULE_DIRTY,
        DIFF_UNTRACKED_UNICODE,
        DIFF_UNTRACKED_BINARY,
        DIFF_UNTRACKED_SYMLINK,
        DIFF_CONFLICT_OURS,
        DIFF_COMBINED,
        DIFF_COMBINED_EVIL,
        DIFF_U0,
        DIFF_WS_MODE,
        b"diff --git a/x b/x\n@@ -1 +1 @@\n-x\n+y",
    ];

    /// Push `buf` into a `PatchSink` in `step`-byte chunks, as `Git::run_streaming` would. Returns the
    /// result and whether the sink stopped the stream before its end.
    fn sink_chunks(buf: &[u8], max: u64, step: usize) -> (FileDiff, bool) {
        let mut sink = PatchSink::new(max);
        let stopped = !buf.chunks(step).all(|chunk| sink.push(chunk));
        (sink.finish(), stopped)
    }

    fn dbg(d: &FileDiff) -> String {
        format!("{d:?}")
    }

    #[test]
    fn chunked_input_parses_like_one_buffer() {
        for (i, buf) in FIXTURES.iter().enumerate() {
            let whole = dbg(&parse_unified(buf));
            let counts = dbg(&Parser::new(false).parse(buf));
            for step in [1, 2, 3, 5, 7, 16, 64, 4096] {
                let mut feed = LineFeed::new(Parser::new(true));
                buf.chunks(step).for_each(|c| feed.feed(c));
                assert_eq!(dbg(&feed.finish()), whole, "fixture {i}, step {step}");
                let mut feed = LineFeed::new(Parser::new(false));
                buf.chunks(step).for_each(|c| feed.feed(c));
                assert_eq!(dbg(&feed.finish()), counts, "fixture {i}, step {step}");
                // Within the limit a sink buffers, then parses exactly like parse_unified.
                let (d, stopped) = sink_chunks(buf, buf.len() as u64, step);
                assert!(!stopped);
                assert_eq!(dbg(&d), whole, "fixture {i}, step {step}");
            }
        }
    }

    #[test]
    fn too_large_starts_one_byte_past_max() {
        let n = DIFF_RENAME.len() as u64;
        let (d, _) = sink_chunks(DIFF_RENAME, n, 10);
        assert!(!d.too_large && d.hunks.len() == 1);
        let (d, stopped) = sink_chunks(DIFF_RENAME, n - 1, 10);
        assert!(d.too_large && d.hunks.is_empty() && !stopped);
        assert_eq!(d.header_lines.len(), 4);
        assert_eq!(d.new_oid.as_deref(), Some("afdb0f7f895ef388c1c50e58a8ac4e73bc561458"));
        assert_eq!((d.additions, d.deletions), (1, 1));
    }

    #[test]
    fn oversized_patch_is_counted_without_being_kept() {
        let mut buf = b"diff --git a/big.csv b/big.csv\n\
            index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
            --- a/big.csv\n\
            +++ b/big.csv\n"
            .to_vec();
        let long = vec![b'x'; 3 * COUNT_LINE_CAP + 17];
        for h in 0..40u32 {
            let s = h * 10 + 1;
            buf.extend_from_slice(format!("@@ -{s},4 +{s},5 @@ ctx {h}\n").as_bytes());
            buf.extend_from_slice(b" keep\n-old\n+new\n");
            if h == 7 {
                // One huge added line: only its prefix is ever held while counting.
                buf.push(b'+');
                buf.extend_from_slice(&long);
                buf.push(b'\n');
            } else {
                buf.extend_from_slice(b"+more\n");
            }
            buf.extend_from_slice(b" keep\n keep\n");
        }
        let whole = parse_unified(&buf);
        assert_eq!((whole.hunks.len(), whole.additions, whole.deletions), (40, 80, 40));
        for step in [1, 7, 1000, 65536] {
            let (d, stopped) = sink_chunks(&buf, 4096, step);
            assert!(d.too_large && d.hunks.is_empty() && !stopped, "step {step}");
            assert_eq!((d.additions, d.deletions), (80, 40), "step {step}");
            assert_eq!(d.header_lines, whole.header_lines);
            assert_eq!((d.old_oid, d.new_oid), (whole.old_oid.clone(), whole.new_oid.clone()));
        }
        // The buffer is released at the switch, and a split line never grows past the cap.
        let mut sink = PatchSink::new(4096);
        for chunk in buf.chunks(1000) {
            assert!(sink.push(chunk));
            if let Some(feed) = &sink.over {
                assert_eq!(sink.buf.capacity(), 0);
                assert!(feed.partial.len() <= COUNT_LINE_CAP);
            }
        }
        assert!(sink.over.is_some());
    }

    #[test]
    fn created_file_settles_from_its_hunk_header() {
        let mut buf = b"diff --git a/new.txt b/new.txt\n\
            new file mode 100644\n\
            index 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333\n\
            --- /dev/null\n\
            +++ b/new.txt\n\
            @@ -0,0 +1,1000 @@\n"
            .to_vec();
        for i in 0..1000 {
            buf.extend_from_slice(format!("+line {i}\n").as_bytes());
        }
        buf.extend_from_slice(b"\\ No newline at end of file\n");
        let whole = parse_unified(&buf);
        assert_eq!(whole.additions, 1000);
        for step in [1, 3, 64, 1000] {
            // Stops as soon as the patch outgrows the limit, with the count from the hunk header.
            let (d, stopped) = sink_chunks(&buf, 300, step);
            assert!(stopped && d.too_large && d.hunks.is_empty(), "step {step}");
            assert_eq!((d.additions, d.deletions), (1000, 0), "step {step}");
            assert_eq!(d.new_mode.as_deref(), Some("100644"));
            assert_eq!(d.header_lines, whole.header_lines);
        }

        // The same hunk shape in an existing file (`-U0` insertion at the top) is read to the end.
        let mut buf = b"diff --git a/keep.txt b/keep.txt\n\
            index de98044..6374de2 100644\n\
            --- a/keep.txt\n\
            +++ b/keep.txt\n\
            @@ -0,0 +1,300 @@\n"
            .to_vec();
        for i in 0..300 {
            buf.extend_from_slice(format!("+top {i}\n").as_bytes());
        }
        buf.extend_from_slice(b"@@ -5,0 +306,2 @@\n+tail 1\n+tail 2\n");
        for step in [1, 7, 4096] {
            let (d, stopped) = sink_chunks(&buf, 100, step);
            assert!(!stopped && d.too_large, "step {step}");
            assert_eq!((d.additions, d.deletions), (302, 0), "step {step}");
        }
    }

    #[test]
    fn type_change_settles_only_in_its_creation_section() {
        let mut buf = b"diff --git a/tc b/tc\n\
            deleted file mode 100644\n\
            index 587be6b4c3f93f93c489c0111bba5596147a26cb..0000000000000000000000000000000000000000\n\
            --- a/tc\n\
            +++ /dev/null\n\
            @@ -1,500 +0,0 @@\n"
            .to_vec();
        for i in 0..500 {
            buf.extend_from_slice(format!("-old {i}\n").as_bytes());
        }
        buf.extend_from_slice(
            b"diff --git a/tc b/tc\n\
            new file mode 120000\n\
            index 0000000000000000000000000000000000000000..1764325aa997b79e6f74da850facef86261812e1\n\
            --- /dev/null\n\
            +++ b/tc\n\
            @@ -0,0 +1,3 @@\n\
            +keep.txt\n\
            +b\n\
            +c\n\
            \\ No newline at end of file\n",
        );
        let whole = parse_unified(&buf);
        assert_eq!((whole.additions, whole.deletions), (3, 500));
        for step in [1, 5, 100] {
            let (d, _) = sink_chunks(&buf, 100, step);
            assert!(d.too_large && d.hunks.is_empty(), "step {step}");
            assert_eq!((d.additions, d.deletions), (3, 500), "step {step}");
            assert_eq!((d.old_mode.as_deref(), d.new_mode.as_deref()), (Some("100644"), Some("120000")));
            assert_eq!(d.header_lines, whole.header_lines);
        }
        // Byte by byte the stream stops inside the creation hunk, never in the deletion.
        let (_, stopped) = sink_chunks(&buf, 100, 1);
        assert!(stopped);
    }

    #[test]
    fn newer_live_request_cancels_the_running_one() {
        let req = |path: &str, context: Option<u32>| DiffRequest {
            target: DiffTarget::Unstaged { cwd: "/live-test-cwd".into() },
            path: path.into(),
            old_path: None,
            context,
            ignore_whitespace: false,
            max_bytes: None,
        };
        let repo = Path::new("/live-test-repo");
        let registered = |r: &LiveRun| LIVE.lock().unwrap().iter().any(|(_, f)| Arc::ptr_eq(f, &r.cancel));
        let a = LiveRun::begin(repo, &req("a.txt", None));
        let other = LiveRun::begin(repo, &req("b.txt", None));
        // The same file again (here with full context): supersedes `a` only.
        let b = LiveRun::begin(repo, &req("a.txt", Some(100000)));
        assert!(a.cancel.load(Ordering::Relaxed));
        assert!(!b.cancel.load(Ordering::Relaxed) && !other.cancel.load(Ordering::Relaxed));
        assert!(!registered(&a) && registered(&b) && registered(&other));
        // The cancelled run ending late leaves the newer one registered.
        drop(a);
        assert!(registered(&b));
        drop(b);
        drop(other);
        assert!(!LIVE.lock().unwrap().iter().any(|(k, _)| k.repo == repo));
    }

    #[test]
    fn order_patterns_match_names_literally() {
        assert_eq!(order_pattern("dir/b.txt").as_deref(), Some("dir/b.txt"));
        // Verified with `git diff-tree -O/dev/stdin`: these lead the output only when escaped.
        assert_eq!(order_pattern("d/z[x]*.txt").as_deref(), Some("d/z\\[x]\\*.txt"));
        assert_eq!(order_pattern("#z.txt").as_deref(), Some("\\#z.txt"));
        assert_eq!(order_pattern("b\\q?.txt").as_deref(), Some("b\\\\q\\?.txt"));
        assert_eq!(order_pattern("a#b.txt").as_deref(), Some("a#b.txt"));
        assert_eq!(order_pattern("한글 #.txt").as_deref(), Some("한글 #.txt"));
        assert_eq!(order_pattern("nl\nname.txt"), None);
    }

    #[test]
    fn hunk_headers() {
        let h = parse_hunk_header(b"@@ -17,7 +17,7 @@ long line 16").unwrap();
        assert_eq!((h.old, h.new), (vec![(17, 7)], (17, 7)));
        let h = parse_hunk_header(b"@@ -2 +2 @@ a  b").unwrap();
        assert_eq!((h.old, h.new), (vec![(2, 1)], (2, 1)));
        let h = parse_hunk_header(b"@@ -0,0 +1 @@").unwrap();
        assert_eq!((h.old, h.new), (vec![(0, 0)], (1, 1)));
        let h = parse_hunk_header(b"@@@ -1,4 -1,4 +1,8 @@@").unwrap();
        assert_eq!((h.old, h.new), (vec![(1, 4), (1, 4)], (1, 8)));
        assert!(parse_hunk_header(b"@@ -1,2 +1,2").is_none());
        assert!(parse_hunk_header(b"@@ -x +1 @@").is_none());
        assert!(parse_hunk_header(b"@@@ -1 +1 @@@").is_none());
        assert!(parse_hunk_header(b"@ -1 +1 @").is_none());
    }
}
