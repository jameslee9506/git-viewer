// Write operations of one repo tab — the only ones the app offers: stage / unstage, commit, branch
// creation, stash push / pop / apply / drop and fetch / pull / push. They run one after the other per
// repo (git takes the index lock, and a pull must not race a commit), surface git's own message in a
// toast when they fail, and ask the store to refresh what they changed (the watcher reports it too).
import { ask } from '@tauri-apps/plugin-dialog';
import { api, errorMessage } from '../api';
import type { ChangeKind, Stash } from '../types';
import type { FileRef, RepoStore } from './repo.svelte';
import { toast } from './ui.svelte';

/** The toolbar operations: one at a time per repo, its button shows a spinner and the others disable. */
export type OpKind = 'fetch' | 'pull' | 'push' | 'branch' | 'stash' | 'pop' | 'apply' | 'drop' | 'commit';

export class WriteOps {
  /** The toolbar operation running, if any (stage / unstage never show here: they are quick). */
  busy = $state<OpKind | null>(null);
  readonly #repo: RepoStore;
  /** Every operation of this repo runs after the previous one. */
  #queue: Promise<unknown> = Promise.resolve();

  constructor(repo: RepoStore) {
    this.#repo = repo;
  }

  /** The paths git needs for these files: both of a rename's / copy's, else the path. */
  static paths(files: readonly FileRef[]): string[] {
    const out: string[] = [];
    for (const f of files) {
      if (f.oldPath != null && f.oldPath !== f.path) out.push(f.oldPath);
      out.push(f.path);
    }
    return out;
  }

  // ───────────── index ─────────────

  /** `git add -A -- <files>` in the worktree at `cwd` (also marks a conflicted file resolved). */
  stage(cwd: string, files: readonly FileRef[]): Promise<boolean> {
    if (files.length === 0) return Promise.resolve(false);
    return this.#run(null, ['index'], () => api.stage(cwd, WriteOps.paths(files)), () => null);
  }

  /** `git add -A`: every change of the worktree, untracked files included. */
  stageAll(cwd: string): Promise<boolean> {
    return this.#run(null, ['index'], () => api.stageAll(cwd), () => null);
  }

  /** `git reset -- <files>`. */
  unstage(cwd: string, files: readonly FileRef[]): Promise<boolean> {
    if (files.length === 0) return Promise.resolve(false);
    return this.#run(null, ['index'], () => api.unstage(cwd, WriteOps.paths(files)), () => null);
  }

  unstageAll(cwd: string): Promise<boolean> {
    return this.#run(null, ['index'], () => api.unstageAll(cwd), () => null);
  }

  /** Commit the index of the worktree at `cwd` with `message` (`amend`: replace the HEAD commit). */
  commit(cwd: string, message: string, amend: boolean): Promise<boolean> {
    return this.#run(
      'commit',
      ['index', 'head', 'refs'],
      () => api.commit(cwd, message, amend),
      () => (amend ? 'Amended' : 'Committed'),
    );
  }

  // ───────────── refs ─────────────

  /** Create `name` at the commit `start`; `checkout` switches to it (only asked for at HEAD). */
  createBranch(name: string, start: string, checkout: boolean): Promise<boolean> {
    const repo = this.#repo;
    return this.#run(
      'branch',
      checkout ? ['refs', 'head'] : ['refs'],
      () => api.createBranch(repo.id, name, start, checkout),
      () => 'Branch created',
    );
  }

  // ───────────── stashes ─────────────

  /** Stash the opened worktree's changes, untracked files included. */
  stash(): Promise<boolean> {
    const repo = this.#repo;
    return this.#run(
      'stash',
      ['worktree', 'index', 'stash'],
      () => api.stashPush(repo.id, true, null),
      (r) => (r.stdout.startsWith('No local changes') ? 'Nothing to stash' : 'Stashed'),
    );
  }

  pop(stash: Stash): Promise<boolean> {
    const repo = this.#repo;
    return this.#run(
      'pop',
      ['worktree', 'index', 'stash'],
      () => api.stashPop(repo.id, stash.index, stash.oid),
      () => 'Popped',
    );
  }

  apply(stash: Stash): Promise<boolean> {
    const repo = this.#repo;
    return this.#run(
      'apply',
      ['worktree', 'index'],
      () => api.stashApply(repo.id, stash.index, stash.oid),
      () => 'Applied',
    );
  }

  /** Drop a stash after a native confirmation (the one destructive operation here). */
  async drop(stash: Stash): Promise<boolean> {
    const repo = this.#repo;
    const ok = await ask(`Drop ${stash.name}?`, { kind: 'warning', okLabel: 'Drop', cancelLabel: 'Cancel' });
    if (!ok) return false;
    return this.#run('drop', ['stash'], () => api.stashDrop(repo.id, stash.index, stash.oid), () => 'Dropped');
  }

  // ───────────── remotes ─────────────

  /** `git fetch --all`. */
  fetch(): Promise<boolean> {
    const repo = this.#repo;
    return this.#run('fetch', ['refs'], () => api.fetch(repo.id, null), () => 'Fetched');
  }

  /** `git pull` of the checked-out branch from its upstream (`rebase`: `--rebase`). */
  pull(rebase: boolean): Promise<boolean> {
    const repo = this.#repo;
    return this.#run(
      'pull',
      ['refs', 'head', 'worktree', 'index'],
      () => api.pull(repo.id, rebase),
      (r) => (/up.to.date/i.test(r.stdout) ? 'Up to date' : 'Pulled'),
    );
  }

  /**
   * Push the checked-out branch: to its upstream, or — without one — to the only remote (else
   * `origin`), setting it as the upstream.
   */
  push(): Promise<boolean> {
    const repo = this.#repo;
    const st = repo.status;
    const branch = st?.head.branch ?? null;
    if (!st || !branch || st.head.detached || st.head.unborn) {
      toast('No branch to push');
      return Promise.resolve(false);
    }
    const remote = st.upstream ? null : this.defaultRemote();
    if (!st.upstream && !remote) {
      toast('No remote');
      return Promise.resolve(false);
    }
    return this.#run(
      'push',
      ['refs'],
      () => api.push(repo.id, remote, remote ? branch : null, remote != null),
      (r) => (/Everything up-to-date/.test(r.stderr) ? 'Up to date' : 'Pushed'),
    );
  }

  /** The remote a branch without an upstream is pushed to: the only one, else `origin`. */
  defaultRemote(): string | null {
    const list = this.#repo.refs?.remoteList ?? [];
    if (list.length === 1) return list[0].name;
    return list.find((r) => r.name === 'origin')?.name ?? null;
  }

  // ───────────── runner ─────────────

  /**
   * Queue `job` after the running operations. `kind` (toolbar operations) sets `busy` meanwhile. On
   * success `done` may name a toast; a failure toasts git's message. Either way `kinds` are refreshed:
   * a pop that stopped on conflicts changed the tree all the same. Resolves true on success.
   */
  #run<T>(kind: OpKind | null, kinds: ChangeKind[], job: () => Promise<T>, done: (r: T) => string | null): Promise<boolean> {
    const repo = this.#repo;
    const p = this.#queue.then(async () => {
      if (kind) this.busy = kind;
      try {
        const r = await job();
        const msg = done(r);
        if (msg) toast(msg);
        return true;
      } catch (e) {
        toast(errorMessage(e));
        return false;
      } finally {
        if (kind && this.busy === kind) this.busy = null;
        void repo.refresh(kinds);
      }
    });
    this.#queue = p;
    return p;
  }
}
