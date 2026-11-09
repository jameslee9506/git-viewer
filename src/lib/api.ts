// Typed wrappers around every Tauri command and event. The only module that calls invoke()/listen().
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  BlameData,
  BranchTrack,
  BlobContent,
  BlobEncoding,
  BlobSpec,
  CommitDetails,
  DiffRequest,
  DiffTarget,
  FileChange,
  FileDiff,
  GraphOptions,
  GraphRow,
  GraphSummary,
  HistoryEntry,
  MenuEvent,
  OpResult,
  RefsSnapshot,
  RepoChanged,
  RepoInfo,
  SearchQuery,
  TreeEntry,
  UserCss,
  WorkingStatus,
} from './types';

export const api = {
  /** Paths passed on the command line at launch (to open as tabs). */
  startupPaths: () => invoke<string[]>('startup_paths'),
  /** The user stylesheet. create=true writes a commented template if the file is missing. */
  userCss: (create = false) => invoke<UserCss>('user_css', { create }),

  /** Resolve any path inside a repo to its root, register it and start watching. */
  openRepo: (path: string) => invoke<RepoInfo>('open_repo', { path }),
  /** Drop caches and stop the watcher for this repo. */
  closeRepo: (repo: string) => invoke<void>('close_repo', { repo }),

  refs: (repo: string) => invoke<RefsSnapshot>('repo_refs', { repo }),
  /** Ahead/behind of every local branch with an existing upstream. `known` = the previous answer
   *  (entries whose tips didn't move are reused without a walk). */
  track: (repo: string, known: BranchTrack[]) => invoke<BranchTrack[]>('repo_track', { repo, known }),
  /** Working tree status of any worktree directory (`cwd`). */
  status: (cwd: string) => invoke<WorkingStatus>('worktree_status', { cwd }),

  graphLoad: (repo: string, opts: GraphOptions, wip: boolean) =>
    invoke<GraphSummary>('graph_load', { repo, opts, wip }),
  /** Rows [start, end) of the given generation. Rejects with 'stale' when generation is outdated. */
  graphRows: (repo: string, generation: number, start: number, end: number) =>
    invoke<GraphRow[]>('graph_rows', { repo, generation, start, end }),
  /** Row index of a full oid, or null when not in the loaded graph. */
  graphFind: (repo: string, generation: number, oid: string) =>
    invoke<number | null>('graph_find', { repo, generation, oid }),
  /** Matching row indices, ascending. */
  graphSearch: (repo: string, generation: number, query: SearchQuery) =>
    invoke<number[]>('graph_search', { repo, generation, query }),

  commitDetails: (repo: string, oid: string) => invoke<CommitDetails>('commit_details', { repo, oid }),
  /** Changed files for range / stash / stashUntracked (also works for commit). */
  diffFiles: (repo: string, target: DiffTarget) => invoke<FileChange[]>('diff_files', { repo, target }),
  fileDiff: (repo: string, req: DiffRequest) => invoke<FileDiff>('file_diff', { repo, req }),

  /** maxBytes null = 16 MiB, 0 = unlimited. Larger blobs come back `truncated` without content. */
  readBlob: (repo: string, spec: BlobSpec, encoding: BlobEncoding, maxBytes: number | null = null) =>
    invoke<BlobContent>('read_blob', { repo, spec, encoding, maxBytes }),
  /** Directory listing at rev; path '' = root. recursive lists all blobs below path. */
  listTree: (repo: string, rev: string, path: string, recursive = false) =>
    invoke<TreeEntry[]>('list_tree', { repo, rev, path, recursive }),

  /** rev null = working tree version (cwd defaults to repo). ignoreWhitespace = `git blame -w`. */
  blame: (repo: string, path: string, rev: string | null, cwd: string | null = null, ignoreWhitespace = false) =>
    invoke<BlameData>('blame_file', { repo, path, rev, cwd, ignoreWhitespace }),
  /** rev null = HEAD. limit 0 = unlimited. */
  fileHistory: (repo: string, path: string, rev: string | null, follow = true, limit = 2000) =>
    invoke<HistoryEntry[]>('file_history', { repo, path, rev, follow, limit }),

  // ───── write operations ─────

  /** `git add -A -- <paths>` in the worktree at `cwd` (a rename passes both of its paths). */
  stage: (cwd: string, paths: string[]) => invoke<void>('wt_stage', { cwd, paths }),
  /** `git add -A`: every change, untracked files included. */
  stageAll: (cwd: string) => invoke<void>('wt_stage_all', { cwd }),
  /** `git reset -q -- <paths>`. */
  unstage: (cwd: string, paths: string[]) => invoke<void>('wt_unstage', { cwd, paths }),
  /** `git reset -q`: the whole index back to HEAD. */
  unstageAll: (cwd: string) => invoke<void>('wt_unstage_all', { cwd }),
  /** `git commit -F -` (`--amend` when asked); the message must not be empty. */
  commit: (cwd: string, message: string, amend: boolean) => invoke<OpResult>('wt_commit', { cwd, message, amend }),
  /** `git branch <name> <start>`, or `git switch -c` when `checkout` (only asked for at the HEAD commit). */
  createBranch: (repo: string, name: string, start: string, checkout: boolean) =>
    invoke<void>('branch_create', { repo, name, start, checkout }),
  /** `git stash push [-u] [-m message]` in the worktree at `cwd`. */
  stashPush: (cwd: string, includeUntracked: boolean, message: string | null) =>
    invoke<OpResult>('stash_push', { cwd, includeUntracked, message }),
  /** `git stash pop stash@{index}`, refused when that entry is no longer `oid`. */
  stashPop: (cwd: string, index: number, oid: string) => invoke<OpResult>('stash_pop', { cwd, index, oid }),
  stashApply: (cwd: string, index: number, oid: string) => invoke<OpResult>('stash_apply', { cwd, index, oid }),
  stashDrop: (cwd: string, index: number, oid: string) => invoke<OpResult>('stash_drop', { cwd, index, oid }),
  /** `git fetch <remote>`, or `--all` with null. */
  fetch: (repo: string, remote: string | null) => invoke<OpResult>('remote_fetch', { repo, remote }),
  /** `git pull --no-edit [--rebase]` of the checked-out branch from its upstream. */
  pull: (repo: string, rebase: boolean) => invoke<OpResult>('remote_pull', { repo, rebase }),
  /** `git push [-u] [<remote> [<branch>]]`: a plain `git push` with every argument null. */
  push: (repo: string, remote: string | null, branch: string | null, setUpstream: boolean) =>
    invoke<OpResult>('remote_push', { repo, remote, branch, setUpstream }),
};

export function onRepoChanged(cb: (e: RepoChanged) => void): Promise<UnlistenFn> {
  return listen<RepoChanged>('repo-changed', (ev) => cb(ev.payload));
}

export function onMenu(cb: (e: MenuEvent) => void): Promise<UnlistenFn> {
  return listen<MenuEvent>('menu', (ev) => cb(ev.payload));
}

export function errorMessage(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

export function isStale(e: unknown): boolean {
  return e === 'stale';
}

/** Folders opened via the Dock icon / `open -a` while running (macOS). */
export function onOpenPaths(cb: (paths: string[]) => void): Promise<UnlistenFn> {
  return listen<string[]>('open-paths', (ev) => cb(ev.payload));
}
