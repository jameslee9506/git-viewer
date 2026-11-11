// Write operations of one repo tab — the only ones the user starts: stage / unstage, commit, branch
// and tag creation and deletion (here and on a remote), checkout, reset, cherry-pick / revert
// (continue / abort), stash push / pop / apply / drop and fetch / pull / push. They run one after the
// other per repo (git takes the index lock, and a pull must not race a commit), surface git's own
// message in a toast when they fail, and ask the store to refresh what they changed (the watcher
// reports it too). The background fetch (autofetch.svelte.ts) runs beside this queue and gives way to
// the network operations here.
import { ask } from '@tauri-apps/plugin-dialog';
import { api, errorMessage } from '../api';
import type {
  ChangeKind,
  CheckoutTarget,
  LocalBranch,
  PickOp,
  PickOutcome,
  RemoteBranch,
  ResetMode,
  Stash,
  Tag,
} from '../types';
import { shortOid } from '../util/format';
import type { FileRef, RepoStore } from './repo.svelte';
import { toast } from './ui.svelte';

/**
 * The operations that hold the repo: one at a time. The toolbar shows a spinner on the running one's
 * button, if it has one there, and disables the others; the footer names it.
 */
export type OpKind =
  | 'fetch'
  | 'pull'
  | 'push'
  | 'branch'
  | 'tag'
  /** The deletion of a branch or a tag, here or on a remote. */
  | 'delete'
  | 'checkout'
  | 'reset'
  /** A cherry-pick, or the continuation of one. */
  | 'pick'
  /** A revert, or the continuation of one. */
  | 'revert'
  /** The abort of a cherry-pick / revert. */
  | 'abort'
  | 'stash'
  | 'pop'
  | 'apply'
  | 'drop'
  | 'commit';

/** What the footer calls a running operation. */
export const OP_LABELS: Record<OpKind, string> = {
  fetch: 'Fetching',
  pull: 'Pulling',
  push: 'Pushing',
  branch: 'Branching',
  tag: 'Tagging',
  delete: 'Deleting',
  checkout: 'Checking out',
  reset: 'Resetting',
  pick: 'Cherry-picking',
  revert: 'Reverting',
  abort: 'Aborting',
  stash: 'Stashing',
  pop: 'Popping',
  apply: 'Applying',
  drop: 'Dropping',
  commit: 'Committing',
};

/** What moving HEAD or committing onto it may change: the files, the index, HEAD, refs, an operation in progress. */
const HEAD_KINDS: ChangeKind[] = ['worktree', 'index', 'head', 'refs', 'state'];

export class WriteOps {
  /** The operation running, if any (stage / unstage never show here: they are quick). */
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

  /** Create the lightweight tag `name` on the commit `oid`. */
  createTag(name: string, oid: string): Promise<boolean> {
    const repo = this.#repo;
    return this.#run('tag', ['refs'], () => api.createTag(repo.id, name, oid), () => 'Tag created');
  }

  // ───────────── deleting refs ─────────────

  /**
   * Ask before a branch or a tag goes (`what`: `main`, `tag v1 from origin`). With `ref` — its full
   * name and the commit it is shown at — the question names the commits that only it reaches: they
   * would be on no branch any more, and out of the graph.
   */
  async #confirmDelete(what: string, ref?: { fullName: string; oid: string }): Promise<boolean> {
    // The count is a safeguard: without it the question is asked all the same.
    const n = ref ? await api.exclusive(this.#repo.id, ref.fullName, ref.oid).catch(() => 0) : 0;
    const left = n > 0 ? ` and leave ${n} commit${n === 1 ? '' : 's'} on no branch` : '';
    return ask(`Delete ${what}${left}?`, { kind: 'warning', okLabel: 'Delete', cancelLabel: 'Cancel' });
  }

  /** A ref deleted here: it leaves the store's hidden and solo lists, then the toast. */
  #deleted(fullName: string): string {
    this.#repo.forgetRefs([fullName]);
    return 'Deleted';
  }

  /**
   * Delete a local branch, merged or not (`git branch -D`). It asks first; git refuses the one that
   * is checked out, here or in another worktree.
   */
  async deleteBranch(b: LocalBranch): Promise<boolean> {
    const repo = this.#repo;
    if (!(await this.#confirmDelete(b.name, b))) return false;
    return this.#run(
      'delete',
      ['refs'],
      () => api.deleteBranch(repo.id, b.name, b.oid),
      () => this.#deleted(b.fullName),
    );
  }

  /** Delete a tag here (`git tag -d`), after asking. */
  async deleteTag(t: Tag): Promise<boolean> {
    const repo = this.#repo;
    if (!(await this.#confirmDelete(`tag ${t.name}`, t))) return false;
    return this.#run('delete', ['refs'], () => api.deleteTag(repo.id, t.name, t.oid), () => this.#deleted(t.fullName));
  }

  /**
   * Delete a branch on its remote, after asking: only while it is there where its remote-tracking
   * branch shows it (one that moved rejects: fetch first). One that is gone there already just loses
   * that remote-tracking branch.
   */
  async deleteRemoteBranch(b: RemoteBranch): Promise<boolean> {
    const repo = this.#repo;
    if (!(await this.#confirmDelete(`${b.short} from ${b.remote}`, b))) return false;
    return this.#run(
      'delete',
      ['refs'],
      () => this.#net(() => api.deleteRemoteBranch(repo.id, b.remote, b.short, b.oid)),
      () => this.#deleted(b.fullName),
    );
  }

  /**
   * Delete the tag `name` on `remote`, after asking. The tag here stays; a remote that has none of
   * that name says so.
   */
  async deleteRemoteTag(name: string, remote: string): Promise<boolean> {
    const repo = this.#repo;
    if (!(await this.#confirmDelete(`tag ${name} from ${remote}`))) return false;
    return this.#run(
      'delete',
      ['refs'],
      () => this.#net(() => api.deleteRemoteTag(repo.id, remote, name)),
      (r) => (/non-existent ref/i.test(r.stderr) ? `Not on ${remote}` : 'Deleted'),
    );
  }

  // ───────────── HEAD ─────────────

  /**
   * Switch the opened worktree to a branch, a new branch tracking a remote one, or a commit (detached
   * HEAD). Away from a detached HEAD, the commits made on it that no branch or tag reaches would be on
   * no ref any more (git only warns; the graph would just lose them): it asks first.
   */
  async checkout(target: CheckoutTarget): Promise<boolean> {
    const repo = this.#repo;
    if (repo.status?.head.detached) {
      const except = target.kind === 'commit' ? target.oid : null;
      // The count is a safeguard: without it the checkout goes ahead, as git's own would.
      const n = await api.unreferenced(repo.id, except).catch(() => 0);
      if (n > 0) {
        const ok = await ask(`Leave ${n} commit${n === 1 ? '' : 's'} on no branch?`, {
          kind: 'warning',
          okLabel: 'Checkout',
          cancelLabel: 'Cancel',
        });
        if (!ok) return false;
      }
    }
    return this.#run('checkout', HEAD_KINDS, () => api.checkout(repo.id, target), () => 'Checked out');
  }

  /**
   * Move the checked-out branch (a detached HEAD itself) to the commit `oid`. A hard reset drops the
   * uncommitted changes of tracked files: it asks first, like a stash drop.
   */
  async reset(oid: string, mode: ResetMode): Promise<boolean> {
    const repo = this.#repo;
    if (mode === 'hard') {
      const st = repo.status;
      const name = st?.head.branch && !st.head.detached ? st.head.branch : 'HEAD';
      // Untracked files stay: only changes of tracked files are lost.
      const tracked = !!st && st.staged.length + st.unstaged.length + st.conflicted.length > 0;
      const ok = await ask(`Hard reset ${name} to ${shortOid(oid)}${tracked ? ' and discard all changes' : ''}?`, {
        kind: 'warning',
        okLabel: 'Reset',
        cancelLabel: 'Cancel',
      });
      if (!ok) return false;
    }
    // A selected commit that the reset takes out of the graph gives way to what it leaves.
    const sel = repo.selection;
    const shown = sel.kind === 'commit' && repo.graph.selectedRow != null;
    return this.#run(
      'reset',
      HEAD_KINDS,
      () => api.reset(repo.id, oid, mode),
      () => 'Reset',
      () => {
        if (shown) void repo.leaveGoneCommit(sel);
      },
    );
  }

  // ───────────── cherry-pick / revert ─────────────

  /** Apply the commit `oid` onto HEAD as a new commit. */
  cherryPick(oid: string): Promise<boolean> {
    return this.#pick('cherry-pick', this.#repo.id, () => api.pick(this.#repo.id, 'cherry-pick', oid));
  }

  /** Undo the commit `oid` with a new commit on HEAD. */
  revert(oid: string): Promise<boolean> {
    return this.#pick('revert', this.#repo.id, () => api.pick(this.#repo.id, 'revert', oid));
  }

  /** Commit the cherry-pick / revert stopped in the worktree at `cwd`, its conflicts resolved. */
  continuePick(cwd: string, op: PickOp): Promise<boolean> {
    return this.#pick(op, cwd, () => api.pickContinue(cwd, op));
  }

  /**
   * Give up the cherry-pick / revert stopped in the worktree at `cwd`: back to where it began. The
   * conflicts resolved so far are lost with it, so it asks first, like a stash drop.
   */
  async abortPick(cwd: string, op: PickOp): Promise<boolean> {
    const ok = await ask(`Abort the ${op}?`, { kind: 'warning', okLabel: 'Abort', cancelLabel: 'Cancel' });
    if (!ok) return false;
    return this.#run('abort', HEAD_KINDS, () => api.pickAbort(cwd, op), () => 'Aborted');
  }

  /**
   * Run a cherry-pick / revert step in the worktree at `cwd`. One that stops on conflicts is no
   * failure: it stays in progress, and once the store has reloaded that worktree's changes are shown,
   * where it is resolved and continued or aborted. Neither is one that found nothing to commit.
   */
  #pick(op: PickOp, cwd: string, job: () => Promise<PickOutcome>): Promise<boolean> {
    const repo = this.#repo;
    const toasts: Record<PickOutcome, string> = {
      done: op === 'revert' ? 'Reverted' : 'Cherry-picked',
      conflicts: 'Conflicts',
      empty: op === 'revert' ? 'Nothing to revert' : 'Nothing to cherry-pick',
    };
    return this.#run(
      op === 'revert' ? 'revert' : 'pick',
      HEAD_KINDS,
      job,
      (r) => toasts[r],
      (r) => {
        if (r === 'conflicts') void repo.showWip(cwd);
      },
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
    return this.#run(
      'fetch',
      ['refs'],
      () => this.#net(() => api.fetch(repo.id, null)),
      () => {
        repo.autoFetch.fetched(repo);
        return 'Fetched';
      },
    );
  }

  /** `git pull` of the checked-out branch from its upstream (`rebase`: `--rebase`). */
  pull(rebase: boolean): Promise<boolean> {
    const repo = this.#repo;
    return this.#run(
      'pull',
      ['refs', 'head', 'worktree', 'index'],
      () =>
        this.#net(async () => {
          // What the pull fetches from, as it starts.
          const remote = this.#upstreamRemote();
          return { remote, out: await api.pull(repo.id, rebase) };
        }),
      (r) => {
        if (r.remote != null) repo.autoFetch.fetched(repo, [r.remote]);
        return /up.to.date/i.test(r.out.stdout) ? 'Up to date' : 'Pulled';
      },
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
      () => this.#net(() => api.push(repo.id, remote, remote ? branch : null, remote != null)),
      (r) => (/Everything up-to-date/.test(r.stderr) ? 'Up to date' : 'Pushed'),
    );
  }

  /** The remote a branch without an upstream is pushed to: the only one, else `origin`. */
  defaultRemote(): string | null {
    const list = this.#repo.refs?.remoteList ?? [];
    if (list.length === 1) return list[0].name;
    return list.find((r) => r.name === 'origin')?.name ?? null;
  }

  /**
   * The remote the checked-out branch's upstream lives on (`origin/main` → `origin`), which a pull
   * fetches. Null without an upstream, or for one that is a local branch.
   */
  #upstreamRemote(): string | null {
    const upstream = this.#repo.status?.upstream;
    if (!upstream) return null;
    return this.#repo.refs?.remotes.find((b) => b.name === upstream)?.remote ?? null;
  }

  /**
   * A job that talks to a remote: the background fetch of the repository gives way first (it would
   * race this one for the same ref locks), and none starts while this operation is `busy`. Not for a
   * tab that closed while the job waited in the queue: the backend stops a network operation with
   * its tab, and under a tab of the repo opened since, nothing would hold the background fetch back
   * for this one.
   */
  async #net<T>(job: () => Promise<T>): Promise<T> {
    if (this.#repo.disposed) throw new Error('cancelled');
    await this.#repo.autoFetch.yield();
    // The tab may have closed while the background fetch came to its end.
    if (this.#repo.disposed) throw new Error('cancelled');
    return job();
  }

  // ───────────── runner ─────────────

  /**
   * Queue `job` after the running operations. `kind` (all but stage / unstage) sets `busy` meanwhile. On
   * success `done` may name a toast; a failure toasts git's message. Either way `kinds` are refreshed:
   * a pop that stopped on conflicts changed the tree all the same. `shown` runs after a success once
   * that refresh is in (the store then shows the outcome: rows, status). Resolves true on success.
   */
  #run<T>(
    kind: OpKind | null,
    kinds: ChangeKind[],
    job: () => Promise<T>,
    done: (r: T) => string | null,
    shown?: (r: T) => void,
  ): Promise<boolean> {
    const repo = this.#repo;
    const p = this.#queue.then(async () => {
      if (kind) this.busy = kind;
      let after: (() => void) | undefined;
      try {
        const r = await job();
        const msg = done(r);
        if (msg) toast(msg);
        if (shown) after = () => shown(r);
        return true;
      } catch (e) {
        toast(errorMessage(e));
        return false;
      } finally {
        if (kind && this.busy === kind) this.busy = null;
        void repo.refresh(kinds).then(after);
      }
    });
    this.#queue = p;
    return p;
  }
}
