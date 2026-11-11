// Typed wrappers around every Tauri command and event. The only module that calls invoke()/listen().
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  BlameData,
  BranchTrack,
  BlobContent,
  BlobEncoding,
  BlobSpec,
  CheckoutTarget,
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
  PickOp,
  PickOutcome,
  RefsSnapshot,
  RemoteFetch,
  RepoChanged,
  RepoInfo,
  ResetMode,
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
  /** `git reset -q -- .`: everything staged back to HEAD (conflicts and an operation in progress stay). */
  unstageAll: (cwd: string) => invoke<void>('wt_unstage_all', { cwd }),
  /** `git commit -F -` (`--amend` when asked); the message must not be empty. */
  commit: (cwd: string, message: string, amend: boolean) => invoke<OpResult>('wt_commit', { cwd, message, amend }),
  /** `git branch <name> <start>`, or `git switch -c` when `checkout` (only asked for at the HEAD commit). */
  createBranch: (repo: string, name: string, start: string, checkout: boolean) =>
    invoke<void>('branch_create', { repo, name, start, checkout }),
  /** `git tag <name> <oid>`: a lightweight tag. */
  createTag: (repo: string, name: string, oid: string) => invoke<void>('tag_create', { repo, name, oid }),
  /** Commits that only the ref `name` (full name, shown at `oid`) reaches: what deleting it would
   *  leave on no branch. Read-only. */
  exclusive: (repo: string, name: string, oid: string) => invoke<number>('ref_exclusive', { repo, name, oid }),
  /** `git branch -D <name>`, refused once the branch is no longer at `oid`. */
  deleteBranch: (repo: string, name: string, oid: string) => invoke<void>('branch_delete', { repo, name, oid }),
  /** `git tag -d <name>`, refused once the tag no longer stands for the commit `oid`. */
  deleteTag: (repo: string, name: string, oid: string) => invoke<void>('tag_delete', { repo, name, oid }),
  /** `git switch` of the opened worktree: a local branch, a new branch tracking a remote one, or a commit (detached). */
  checkout: (repo: string, target: CheckoutTarget) => invoke<void>('head_checkout', { repo, target }),
  /** `git reset --soft|--mixed|--hard <oid>`: moves the checked-out branch (a detached HEAD itself). */
  reset: (repo: string, oid: string, mode: ResetMode) => invoke<void>('head_reset', { repo, oid, mode }),
  /** Commits on HEAD that no ref reaches (nor `except`, the commit about to be checked out): what a
   *  checkout away from a detached HEAD would leave on no branch. */
  unreferenced: (repo: string, except: string | null) => invoke<number>('head_unreferenced', { repo, except }),
  /** `git cherry-pick <oid>` / `git revert --no-edit <oid>` onto HEAD (a merge commit against its first
   *  parent): 'conflicts' leaves it in progress, 'empty' found nothing to commit. Rejects while
   *  another operation is in progress. */
  pick: (repo: string, op: PickOp, oid: string) => invoke<PickOutcome>('commit_pick', { repo, op, oid }),
  /** `git cherry-pick|revert --continue` in the worktree at `cwd`, once its conflicts are resolved (a
   *  commit left with nothing to commit is skipped). */
  pickContinue: (cwd: string, op: PickOp) => invoke<PickOutcome>('wt_pick_continue', { cwd, op }),
  /** `git cherry-pick|revert --abort` in the worktree at `cwd`. */
  pickAbort: (cwd: string, op: PickOp) => invoke<void>('wt_pick_abort', { cwd, op }),
  /** `git stash push [-u] [-m message]` in the worktree at `cwd`. */
  stashPush: (cwd: string, includeUntracked: boolean, message: string | null) =>
    invoke<OpResult>('stash_push', { cwd, includeUntracked, message }),
  /** `git stash pop stash@{index}`, refused when that entry is no longer `oid`. */
  stashPop: (cwd: string, index: number, oid: string) => invoke<OpResult>('stash_pop', { cwd, index, oid }),
  stashApply: (cwd: string, index: number, oid: string) => invoke<OpResult>('stash_apply', { cwd, index, oid }),
  stashDrop: (cwd: string, index: number, oid: string) => invoke<OpResult>('stash_drop', { cwd, index, oid }),
  /** `git fetch <remote>`, or `--all` with null. */
  fetch: (repo: string, remote: string | null) => invoke<OpResult>('remote_fetch', { repo, remote }),
  /** The background fetch: `git fetch <remote>` for each of `remotes` in turn, without ever prompting
   *  and without touching `FETCH_HEAD`. A remote that fails is no rejection: its entry carries git's
   *  message. Cut short (`cancelFetchBackground`, the tab closed), it leaves out the remotes it did
   *  not get to. */
  fetchBackground: (repo: string, remotes: string[]) =>
    invoke<RemoteFetch[]>('remote_fetch_background', { repo, remotes }),
  /** Stop the background fetch running through `repo`, if any. */
  cancelFetchBackground: (repo: string) => invoke<void>('remote_fetch_cancel', { repo }),
  /** `git pull --no-edit [--rebase]` of the checked-out branch from its upstream. */
  pull: (repo: string, rebase: boolean) => invoke<OpResult>('remote_pull', { repo, rebase }),
  /** `git push [-u] [<remote> [<branch>]]`: a plain `git push` with every argument null. */
  push: (repo: string, remote: string | null, branch: string | null, setUpstream: boolean) =>
    invoke<OpResult>('remote_push', { repo, remote, branch, setUpstream }),
  /** Delete `branch` on `remote` (`git push --force-with-lease`): only while it is at `oid` there. One
   *  that is gone there already loses its remote-tracking branch here; one that moved rejects. */
  deleteRemoteBranch: (repo: string, remote: string, branch: string, oid: string) =>
    invoke<void>('remote_delete_branch', { repo, remote, branch, oid }),
  /** Delete the tag `name` on `remote` (`git push <remote> :refs/tags/<name>`); a remote without it
   *  only warns, in `stderr`. */
  deleteRemoteTag: (repo: string, remote: string, name: string) =>
    invoke<OpResult>('remote_delete_tag', { repo, remote, name }),
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
