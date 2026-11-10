//! IPC contract: every type crossing the Tauri boundary. Mirrored 1:1 in `src/lib/types.ts`.
//! All structs serialize with camelCase field names. Keep both files in sync.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ───────────────────────────── Repository ─────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    /// Canonical absolute path of the opened worktree root (bare repo: the git dir). This is the repo id.
    pub path: String,
    /// Last path component of `path`.
    pub name: String,
    /// Absolute git dir of this worktree (`.git` or `.git/worktrees/<name>`).
    pub git_dir: String,
    /// Absolute common dir shared by all worktrees.
    pub common_dir: String,
    pub is_bare: bool,
    /// `git_dir != common_dir`.
    pub is_linked_worktree: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct HeadInfo {
    /// None when unborn.
    pub oid: Option<String>,
    /// Short branch name when HEAD is a symbolic ref (also set when unborn).
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
}

// ───────────────────────────── Refs ─────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LocalBranch {
    /// Short name, e.g. `feature/x`.
    pub name: String,
    /// `refs/heads/feature/x`.
    pub full_name: String,
    pub oid: String,
    /// Short upstream name, e.g. `origin/feature/x`.
    pub upstream: Option<String>,
    /// 0 from `repo_refs`; filled from `repo_track` by the frontend.
    pub ahead: u32,
    pub behind: u32,
    /// Upstream configured but the remote ref no longer exists.
    pub upstream_gone: bool,
    /// Checked out in the opened worktree.
    pub is_head: bool,
    /// Path of the worktree where this branch is checked out (any worktree), if any.
    pub worktree: Option<String>,
    /// Committer time of the tip (unix seconds).
    pub time: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBranch {
    /// Remote name, e.g. `origin`.
    pub remote: String,
    /// `origin/main`.
    pub name: String,
    /// Branch part without the remote, e.g. `main` or `feature/x`.
    pub short: String,
    /// `refs/remotes/origin/main`.
    pub full_name: String,
    pub oid: String,
    pub time: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub url: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    pub full_name: String,
    /// Peeled target oid: a commit unless `target_type` says otherwise.
    pub oid: String,
    /// Object type of the peeled target: "commit" | "tree" | "blob" ("tag" for a tag of a tag when git
    /// peels `%(*…)` atoms one level only, as older versions do). Only commit tags are in the graph.
    pub target_type: String,
    pub annotated: bool,
    /// Annotated tag subject.
    pub message: Option<String>,
    /// Tagger time for annotated tags, else committer time of the target (0 when that is a tree or blob).
    pub time: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Stash {
    /// N in `stash@{N}`.
    pub index: u32,
    /// `stash@{N}`.
    pub name: String,
    pub oid: String,
    /// First parent: the commit the stash was created on.
    pub base: String,
    /// Second parent: index state commit.
    pub index_commit: Option<String>,
    /// Third parent: untracked files commit (only with `stash -u`).
    pub untracked_commit: Option<String>,
    pub message: String,
    /// Author name and email of the stash commit (mailmap applied, as for graph commits).
    pub author: String,
    pub email: String,
    /// Author time.
    pub time: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub head: Option<String>,
    /// Short branch name.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
    /// First entry of `git worktree list` (the main worktree).
    pub is_main: bool,
    /// The worktree that was opened (RepoInfo.path).
    pub is_current: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Submodule {
    pub path: String,
    /// Commit recorded in the superproject index ("ours" when the gitlink is unmerged).
    pub oid: String,
    /// "ok" | "uninitialized" | "conflict". A checkout that differs from `oid` is not reported
    /// here: the WIP status lists it (an unstaged `StatusEntry` with `submodule`).
    pub state: String,
}

/// Ahead/behind of a local branch against its upstream (`repo_track`, computed lazily: refs snapshots
/// carry 0 in `LocalBranch.ahead/behind` and the frontend fills them from this).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchTrack {
    /// Full ref name, e.g. `refs/heads/main`.
    pub name: String,
    /// Branch tip the counts are for.
    pub oid: String,
    /// Upstream tip they are against.
    pub upstream_oid: String,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RefsSnapshot {
    pub head: HeadInfo,
    pub locals: Vec<LocalBranch>,
    pub remotes: Vec<RemoteBranch>,
    pub remote_list: Vec<Remote>,
    pub tags: Vec<Tag>,
    pub stashes: Vec<Stash>,
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
}

// ───────────────────────────── Graph ─────────────────────────────

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GraphOrder {
    Date,
    Topo,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphOptions {
    pub order: GraphOrder,
    /// Max commits to load; 0 = unlimited.
    pub limit: u32,
    pub include_remotes: bool,
    pub include_tags: bool,
    pub include_stashes: bool,
    /// Full ref names excluded from the walk (GitKraken "hide").
    pub hidden_refs: Vec<String>,
    /// When non-empty, walk only these full ref names ("solo"). HEAD is still included.
    pub only_refs: Vec<String>,
    /// Keep HEAD's first-parent chain in lane 0.
    pub pin_head_lane: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GraphSummary {
    /// Identifies the installed graph; row/search calls must pass it back. A load that changes nothing
    /// (same walk, tips, synthetic rows and HEAD) returns the current generation.
    pub generation: u64,
    pub total: u32,
    /// Widest lane count over all rows.
    pub max_lanes: u32,
    pub head_row: Option<u32>,
    pub wip_row: Option<u32>,
    /// The commit limit was hit (older history not loaded).
    pub truncated: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RowKind {
    Commit,
    Wip,
    Stash,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RefLabelKind {
    Head,
    Local,
    Remote,
    Tag,
    Stash,
    Worktree,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub kind: RefLabelKind,
    /// Display text: short branch/tag name, `stash@{0}`, worktree folder name, or `HEAD`.
    pub name: String,
    /// Full ref name (`refs/heads/x`), or the worktree path for worktree labels.
    pub full_name: String,
    /// The branch checked out in the opened worktree.
    pub current: bool,
}

/// Edge from this row's center to the next row's center: `[fromLane, toLane, color, flags]`.
/// `color` is a palette index (`0..8`); `flags` is a bit set of the `EDGE_*` constants.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge(pub u16, pub u16, pub u8, pub u8);

/// Dashed line (WIP / stash link and its continuation).
pub const EDGE_DASHED: u8 = 1;
/// Starts at this row's node (otherwise it is a line passing through).
pub const EDGE_FROM_NODE: u8 = 2;
/// Ends at the next row's node.
pub const EDGE_TO_NODE: u8 = 4;
/// Link from a merge commit to its 2nd+ parent.
pub const EDGE_MERGE: u8 = 8;
/// Leaves the last loaded row towards a parent that is not loaded (truncated history).
pub const EDGE_OPEN: u8 = 16;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    pub kind: RowKind,
    /// Full oid; empty string for the WIP row.
    pub oid: String,
    /// Parent oids as shown in the graph (WIP: [HEAD]; stash: [base]).
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub email: String,
    /// Author time, unix seconds (WIP: 0).
    pub time: i64,
    pub lane: u16,
    pub color: u16,
    /// Edges leaving this row towards row+1 (the last row's edges point past the end = truncated history).
    pub edges: Vec<Edge>,
    pub refs: Vec<RefLabel>,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// In-memory: subject, author name/email, oid prefix, ref names (case-insensitive substring).
    Quick,
    /// `git log --grep` over full messages (fixed string, case-insensitive).
    Message,
    /// Commits touching paths matching the text (pathspec, glob allowed).
    Path,
    /// Pickaxe `-S` content search.
    Content,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: String,
    pub mode: SearchMode,
}

// ───────────────────────────── Commits & files ─────────────────────────────

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub name: String,
    pub email: String,
    pub time: i64,
    /// Offset like `+0900`.
    pub tz: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// Source path for renames/copies.
    pub old_path: Option<String>,
    /// One of `A M D R C T U` (and `?` for untracked stash files).
    pub status: String,
    /// None for binary files.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
    /// Rename/copy similarity percent.
    pub similarity: Option<u32>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// Gitlink (mode 160000).
    pub submodule: bool,
    /// Git LFS tracks the path (`filter=lfs` attribute): its blob is a pointer file.
    pub lfs: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: Person,
    pub committer: Person,
    /// First line of the message.
    pub subject: String,
    /// Remaining message, trimmed (may be empty).
    pub body: String,
    /// Changes against the first parent (root commit: against the empty tree).
    pub files: Vec<FileChange>,
    pub additions: u32,
    pub deletions: u32,
}

// ───────────────────────────── Diffs ─────────────────────────────

/// What to diff. Internally tagged: `{ "kind": "commit", "oid": "...", "parent": null }`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffTarget {
    /// `parent..oid`; parent None = first parent (or empty tree for a root commit).
    Commit { oid: String, parent: Option<String> },
    /// `from..to` (two-dot, tree comparison).
    Range { from: String, to: String },
    /// HEAD vs index in the worktree at `cwd`.
    Staged { cwd: String },
    /// Index vs working tree at `cwd`.
    Unstaged { cwd: String },
    /// Untracked file vs nothing at `cwd`.
    Untracked { cwd: String },
    /// Unmerged path at `cwd`: "ours" (stage 2) vs working tree.
    Conflicted { cwd: String },
    /// Commit `oid` vs the working tree at `cwd` (`git diff <oid>`): staged and unstaged changes together.
    /// Untracked files are not included (list them with `Untracked`).
    Worktree { cwd: String, oid: String },
    /// Tracked changes of a stash: base..stash.
    Stash { oid: String },
    /// Files recorded in the stash's untracked commit (3rd parent).
    StashUntracked { oid: String },
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiffRequest {
    pub target: DiffTarget,
    pub path: String,
    pub old_path: Option<String>,
    /// Context lines; None = 3. Large values (e.g. 100000) show the full file.
    pub context: Option<u32>,
    pub ignore_whitespace: bool,
    /// Diff text bytes above which `too_large` is returned without hunks; None = 8 MiB. 0 = unlimited.
    pub max_bytes: Option<u64>,
}

/// One diff line: `[kind, oldLineNo, newLineNo, text]`; kind 0 = context, 1 = added, 2 = deleted,
/// 3 = "\ No newline at end of file" marker (line numbers null, text empty).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DiffLine(pub u8, pub Option<u32>, pub Option<u32>, pub String);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// Full `@@ -a,b +c,d @@ section` line.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub binary: bool,
    pub too_large: bool,
    /// Blob ids from the `index` header line when present (abbreviated or full).
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// Extended header lines between `diff --git` and `---` (e.g. `new file mode 100644`, `similarity index 90%`).
    pub header_lines: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub additions: u32,
    pub deletions: u32,
}

// ───────────────────────────── Working tree ─────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub path: String,
    pub old_path: Option<String>,
    /// Single letter for this side: `A M D R C T` (staged/unstaged), `?` (untracked), `U` (conflicted).
    pub status: String,
    /// Two-letter unmerged code for conflicts (`UU`, `AA`, `DU`, ...).
    pub conflict: Option<String>,
    pub submodule: bool,
    /// Git LFS tracks the path (`filter=lfs` attribute in the worktree's rules).
    pub lfs: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoOperation {
    /// "merge" | "rebase" | "cherry-pick" | "revert" | "bisect" | "am"
    pub kind: String,
    pub step: Option<u32>,
    pub total: Option<u32>,
    /// e.g. branch being rebased or MERGE_MSG first line.
    pub detail: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkingStatus {
    pub cwd: String,
    pub head: HeadInfo,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<StatusEntry>,
    pub unstaged: Vec<StatusEntry>,
    pub untracked: Vec<StatusEntry>,
    pub conflicted: Vec<StatusEntry>,
    pub operation: Option<RepoOperation>,
}

// ───────────────────────────── Write operations ─────────────────────────────

/// stdout / stderr of a write operation that succeeded (trimmed; the UI shows a terse toast from them).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    pub stdout: String,
    pub stderr: String,
}

/// What a checkout switches the worktree to. Internally tagged: `{ "kind": "branch", "name": "main" }`.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CheckoutTarget {
    /// A local branch (short name).
    Branch { name: String },
    /// A remote branch that has no local one yet: the new local branch `name`, tracking `upstream`
    /// (full ref name, `refs/remotes/origin/x`).
    Track { name: String, upstream: String },
    /// A commit, leaving HEAD detached.
    Commit { oid: String },
}

/// `git reset` mode: soft keeps the index and the files, mixed the files, hard neither.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

/// The sequencer operations the app starts, continues and aborts; named like `RepoOperation::kind`.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PickOp {
    CherryPick,
    Revert,
}

/// How a cherry-pick / revert (or its `--continue`) ended: its commit was made, or it stopped on
/// conflicts and stays in progress.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PickOutcome {
    Done,
    Conflicts,
}

// ───────────────────────────── Blobs / files ─────────────────────────────

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BlobSpec {
    /// `<rev>:<path>`; rev may also be an index stage `:0`–`:3` (reads `:<n>:<path>`).
    Rev { rev: String, path: String },
    /// Stage-0 index entry in the worktree at `cwd` (stage 2 "ours" when the path is unmerged).
    Index { cwd: String, path: String },
    /// File on disk inside the worktree at `cwd`.
    Worktree { cwd: String, path: String },
    /// Blob by object id.
    Oid { oid: String },
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BlobEncoding {
    /// Decode as UTF-8 (lossy); binary content returns neither text nor base64.
    Text,
    /// Always return base64 (images).
    Base64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BlobContent {
    /// Full size in bytes.
    pub size: u64,
    /// NUL byte found in the first 8000 bytes.
    pub binary: bool,
    /// Content cut at max_bytes.
    pub truncated: bool,
    pub text: Option<String>,
    pub base64: Option<String>,
    /// Guessed from the extension (images), e.g. `image/png`.
    pub mime: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    /// Path relative to the repo root.
    pub path: String,
    /// "blob" | "tree" | "commit"
    pub kind: String,
    pub mode: String,
    pub oid: String,
    /// Blob size; None for trees/submodules.
    pub size: Option<u64>,
}

// ───────────────────────────── Blame / history ─────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BlameCommit {
    pub oid: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    /// All-zero oid: lines not yet committed.
    pub uncommitted: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BlameHunk {
    /// 0-based first line in `lines`.
    pub start: u32,
    pub count: u32,
    pub oid: String,
    /// Path of the line in that commit when it differs (renames).
    pub orig_path: Option<String>,
    /// 1-based line number in that commit.
    pub orig_line: u32,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BlameData {
    pub path: String,
    pub lines: Vec<String>,
    /// Sorted by start, covering every line exactly once.
    pub hunks: Vec<BlameHunk>,
    pub commits: HashMap<String, BlameCommit>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub subject: String,
    /// Path of the file in this commit (differs before a rename when following).
    pub path: String,
    pub old_path: Option<String>,
    /// `A M D R C T`.
    pub status: String,
}

// ───────────────────────────── Events ─────────────────────────────

/// Payload of the `repo-changed` event.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoChanged {
    /// RepoInfo.path of the affected repo.
    pub repo: String,
    /// Subset of: "worktree" | "index" | "head" | "refs" | "stash" | "state" | "config".
    pub kinds: Vec<String>,
}

/// Payload of the `menu` event.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MenuEvent {
    /// "open" | "new-tab" | "close-tab" | "refresh" | "find" | "toggle-details" | "settings" |
    /// "zoom-in" | "zoom-out" | "zoom-reset"
    pub id: String,
}

/// User stylesheet loaded after the built-in theme (`<app config dir>/user.css`).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserCss {
    pub path: String,
    /// None when the file does not exist.
    pub css: Option<String>,
}
