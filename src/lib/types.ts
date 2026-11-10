// IPC contract. Mirrors src-tauri/src/model.rs 1:1 (camelCase). Keep both files in sync.

// ───────────── Repository ─────────────

export interface RepoInfo {
  /** Canonical worktree root (bare: git dir). Used as the repo id everywhere. */
  path: string;
  name: string;
  gitDir: string;
  commonDir: string;
  isBare: boolean;
  isLinkedWorktree: boolean;
}

export interface HeadInfo {
  oid: string | null;
  branch: string | null;
  detached: boolean;
  unborn: boolean;
}

// ───────────── Refs ─────────────

export interface LocalBranch {
  name: string;
  fullName: string;
  oid: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  upstreamGone: boolean;
  isHead: boolean;
  worktree: string | null;
  time: number;
}

export interface RemoteBranch {
  remote: string;
  name: string;
  short: string;
  fullName: string;
  oid: string;
  time: number;
}

export interface Remote {
  name: string;
  url: string | null;
}

export interface Tag {
  name: string;
  fullName: string;
  /** Peeled target oid: a commit unless `targetType` says otherwise. */
  oid: string;
  /** Peeled target type; only commit tags are in the graph ('tag': a tag of a tag peeled one level by older git). */
  targetType: 'commit' | 'tree' | 'blob' | 'tag';
  annotated: boolean;
  message: string | null;
  /** Tagger time (annotated), else the target's committer time (0 when that is a tree or blob). */
  time: number;
}

export interface Stash {
  index: number;
  name: string;
  oid: string;
  base: string;
  indexCommit: string | null;
  untrackedCommit: string | null;
  message: string;
  /** Author of the stash commit (mailmap applied). */
  author: string;
  email: string;
  /** author time, unix seconds */
  time: number;
}

export interface Worktree {
  path: string;
  head: string | null;
  branch: string | null;
  detached: boolean;
  bare: boolean;
  locked: boolean;
  prunable: boolean;
  isMain: boolean;
  isCurrent: boolean;
}

export interface Submodule {
  path: string;
  /** Commit recorded in the superproject index ("ours" when the gitlink is unmerged). */
  oid: string;
  /** A checkout that differs from `oid` is not reported here: the WIP status lists it (an unstaged `StatusEntry` with `submodule`). */
  state: 'ok' | 'uninitialized' | 'conflict';
}

/** Ahead/behind of a local branch against its upstream (`repo_track`; computed lazily, refs snapshots
 *  carry 0). */
export interface BranchTrack {
  /** Full ref name, e.g. `refs/heads/main`. */
  name: string;
  /** Branch tip the counts are for. */
  oid: string;
  /** Upstream tip they are against. */
  upstreamOid: string;
  ahead: number;
  behind: number;
}

export interface RefsSnapshot {
  head: HeadInfo;
  locals: LocalBranch[];
  remotes: RemoteBranch[];
  remoteList: Remote[];
  tags: Tag[];
  stashes: Stash[];
  worktrees: Worktree[];
  submodules: Submodule[];
}

// ───────────── Graph ─────────────

export type GraphOrder = 'date' | 'topo';

export interface GraphOptions {
  order: GraphOrder;
  /** 0 = unlimited */
  limit: number;
  includeRemotes: boolean;
  includeTags: boolean;
  includeStashes: boolean;
  /** Full ref names excluded from the walk. */
  hiddenRefs: string[];
  /** When non-empty, only these full ref names (+HEAD) are walked. */
  onlyRefs: string[];
  pinHeadLane: boolean;
}

export interface GraphSummary {
  generation: number;
  total: number;
  maxLanes: number;
  headRow: number | null;
  wipRow: number | null;
  truncated: boolean;
}

export type RowKind = 'commit' | 'wip' | 'stash';
export type RefLabelKind = 'head' | 'local' | 'remote' | 'tag' | 'stash' | 'worktree';

export interface RefLabel {
  kind: RefLabelKind;
  name: string;
  fullName: string;
  current: boolean;
}

/** [fromLane, toLane, color (0..7), flags] from this row's center to the next row's center. */
export type Edge = [from: number, to: number, color: number, flags: number];

/** Edge flag bits (mirror of model.rs EDGE_*). */
export const EDGE_DASHED = 1;
export const EDGE_FROM_NODE = 2;
export const EDGE_TO_NODE = 4;
export const EDGE_MERGE = 8;
export const EDGE_OPEN = 16;

export interface GraphRow {
  kind: RowKind;
  /** '' for the WIP row */
  oid: string;
  parents: string[];
  subject: string;
  author: string;
  email: string;
  /** author time, unix seconds (WIP: 0) */
  time: number;
  lane: number;
  color: number;
  edges: Edge[];
  refs: RefLabel[];
}

export type SearchMode = 'quick' | 'message' | 'path' | 'content';

export interface SearchQuery {
  text: string;
  mode: SearchMode;
}

// ───────────── Commits & files ─────────────

export interface Person {
  name: string;
  email: string;
  time: number;
  tz: string;
}

export type ChangeStatus = 'A' | 'M' | 'D' | 'R' | 'C' | 'T' | 'U' | '?';

export interface FileChange {
  path: string;
  oldPath: string | null;
  status: ChangeStatus;
  additions: number | null;
  deletions: number | null;
  binary: boolean;
  similarity: number | null;
  oldMode: string | null;
  newMode: string | null;
  submodule: boolean;
  /** Git LFS tracks the path (`filter=lfs` attribute): its blob is a pointer file. */
  lfs: boolean;
}

export interface CommitDetails {
  oid: string;
  parents: string[];
  author: Person;
  committer: Person;
  subject: string;
  body: string;
  files: FileChange[];
  additions: number;
  deletions: number;
}

// ───────────── Diffs ─────────────

export type DiffTarget =
  | { kind: 'commit'; oid: string; parent: string | null }
  | { kind: 'range'; from: string; to: string }
  | { kind: 'staged'; cwd: string }
  | { kind: 'unstaged'; cwd: string }
  | { kind: 'untracked'; cwd: string }
  | { kind: 'conflicted'; cwd: string }
  /** Commit `oid` vs the working tree at `cwd` (staged + unstaged; untracked files are listed separately). */
  | { kind: 'worktree'; cwd: string; oid: string }
  | { kind: 'stash'; oid: string }
  | { kind: 'stashUntracked'; oid: string };

export interface DiffRequest {
  target: DiffTarget;
  path: string;
  oldPath: string | null;
  /** null = 3 */
  context: number | null;
  ignoreWhitespace: boolean;
  /** null = 8 MiB, 0 = unlimited */
  maxBytes: number | null;
}

/** 0 context, 1 added, 2 deleted, 3 "\ No newline at end of file" */
export type DiffLineKind = 0 | 1 | 2 | 3;
export type DiffLine = [kind: DiffLineKind, oldNo: number | null, newNo: number | null, text: string];

export interface Hunk {
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface FileDiff {
  path: string;
  oldPath: string | null;
  binary: boolean;
  tooLarge: boolean;
  oldOid: string | null;
  newOid: string | null;
  oldMode: string | null;
  newMode: string | null;
  headerLines: string[];
  hunks: Hunk[];
  additions: number;
  deletions: number;
}

// ───────────── Working tree ─────────────

export interface StatusEntry {
  path: string;
  oldPath: string | null;
  status: ChangeStatus;
  conflict: string | null;
  submodule: boolean;
  /** Git LFS tracks the path (`filter=lfs` attribute in the worktree's rules). */
  lfs: boolean;
}

export interface RepoOperation {
  kind: 'merge' | 'rebase' | 'cherry-pick' | 'revert' | 'bisect' | 'am';
  step: number | null;
  total: number | null;
  detail: string | null;
}

export interface WorkingStatus {
  cwd: string;
  head: HeadInfo;
  upstream: string | null;
  ahead: number;
  behind: number;
  staged: StatusEntry[];
  unstaged: StatusEntry[];
  untracked: StatusEntry[];
  conflicted: StatusEntry[];
  operation: RepoOperation | null;
}

// ───────────── Blobs / files ─────────────

export type BlobSpec =
  /** rev may also be an index stage ':0'–':3' */
  | { kind: 'rev'; rev: string; path: string }
  /** stage 0, or stage 2 ("ours") when the path is unmerged */
  | { kind: 'index'; cwd: string; path: string }
  | { kind: 'worktree'; cwd: string; path: string }
  | { kind: 'oid'; oid: string };

/** 'text': UTF-8 text unless binary (binary → neither text nor base64); 'base64': always bytes. */
export type BlobEncoding = 'text' | 'base64';

export interface BlobContent {
  size: number;
  binary: boolean;
  truncated: boolean;
  text: string | null;
  base64: string | null;
  mime: string | null;
}

export interface TreeEntry {
  name: string;
  path: string;
  kind: 'blob' | 'tree' | 'commit';
  mode: string;
  oid: string;
  size: number | null;
}

// ───────────── Blame / history ─────────────

export interface BlameCommit {
  oid: string;
  author: string;
  email: string;
  time: number;
  summary: string;
  uncommitted: boolean;
}

export interface BlameHunk {
  start: number;
  count: number;
  oid: string;
  origPath: string | null;
  origLine: number;
}

export interface BlameData {
  path: string;
  lines: string[];
  hunks: BlameHunk[];
  commits: Record<string, BlameCommit>;
}

export interface HistoryEntry {
  oid: string;
  parents: string[];
  author: string;
  email: string;
  time: number;
  subject: string;
  path: string;
  oldPath: string | null;
  status: ChangeStatus;
}

// ───────────── Write operations ─────────────

/** stdout / stderr (trimmed) of a write operation that succeeded; the UI derives a terse toast from them. */
export interface OpResult {
  stdout: string;
  stderr: string;
}

/** How the background fetch of one remote ended (`remote_fetch_background`). */
export interface RemoteFetch {
  remote: string;
  /** Not fetched: `git fetch --all` skips this remote too (`remote.<name>.skipFetchAll`). */
  skipped: boolean;
  /** git reported ref updates: new, moved or pruned branches and tags. */
  updated: boolean;
  /** git's message when the fetch failed. */
  error: string | null;
}

/** What a checkout switches the worktree to. */
export type CheckoutTarget =
  /** A local branch (short name). */
  | { kind: 'branch'; name: string }
  /** A remote branch that has no local one yet: the new local branch `name`, tracking `upstream` (full ref name). */
  | { kind: 'track'; name: string; upstream: string }
  /** A commit, leaving HEAD detached. */
  | { kind: 'commit'; oid: string };

/** `git reset` mode: soft keeps the index and the files, mixed the files, hard neither. */
export type ResetMode = 'soft' | 'mixed' | 'hard';

/** The sequencer operations the app starts, continues and aborts (a `RepoOperation['kind']`). */
export type PickOp = 'cherry-pick' | 'revert';

/**
 * How a cherry-pick / revert (or its continue) ended: committed; stopped on conflicts (still in
 * progress); or over without a commit, as there was nothing to commit.
 */
export type PickOutcome = 'done' | 'conflicts' | 'empty';

// ───────────── Events ─────────────

export type ChangeKind = 'worktree' | 'index' | 'head' | 'refs' | 'stash' | 'state' | 'config';

export interface RepoChanged {
  repo: string;
  kinds: ChangeKind[];
}

export type MenuId =
  | 'open'
  | 'new-tab'
  | 'close-tab'
  | 'refresh'
  | 'find'
  | 'toggle-details'
  | 'settings'
  | 'zoom-in'
  | 'zoom-out'
  | 'zoom-reset';

export interface MenuEvent {
  id: MenuId;
}

export interface UserCss {
  path: string;
  css: string | null;
}
