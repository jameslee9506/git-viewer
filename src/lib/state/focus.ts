// The branch the simple view follows: the one picked in its sidebar, else the repository's main
// branch. Pure functions over a refs snapshot (no Svelte).
import type { LocalBranch, RefsSnapshot, RemoteBranch } from '../types';

/** A branch the simple view follows: a local branch with its upstream, or a remote branch alone. */
export interface Focus {
  /** Short name: `main`, `feature/x` (a remote branch's without its remote). */
  name: string;
  local: LocalBranch | null;
  /** The local branch's upstream when that is a remote branch here, else the remote branch followed. */
  remote: RemoteBranch | null;
  /** Full names of the refs walked: the local branch, then the remote one. */
  refs: string[];
}

/** Names taken for the main branch when no remote names its default branch (`<remote>/HEAD` unset). */
const MAIN_NAMES = ['main', 'master'];

/** The remote branch a local branch tracks, when it exists. */
export function upstreamOf(refs: RefsSnapshot, b: LocalBranch): RemoteBranch | null {
  if (!b.upstream || b.upstreamGone) return null;
  return refs.remotes.find((r) => r.name === b.upstream) ?? null;
}

/** The local branch of a remote branch: the one of its name that tracks it, else one of its name that tracks nothing. */
export function localOf(refs: RefsSnapshot, r: RemoteBranch): LocalBranch | null {
  return (
    refs.locals.find((l) => l.name === r.short && l.upstream === r.name) ??
    refs.locals.find((l) => l.name === r.short && l.upstream == null) ??
    null
  );
}

function ofLocal(refs: RefsSnapshot, local: LocalBranch): Focus {
  const remote = upstreamOf(refs, local);
  return { name: local.name, local, remote, refs: remote ? [local.fullName, remote.fullName] : [local.fullName] };
}

/** A remote branch with its local branch (`localOf`; also one that tracks nothing: the two are walked together). */
function ofRemote(refs: RefsSnapshot, remote: RemoteBranch): Focus {
  const local = localOf(refs, remote);
  if (local) return { name: local.name, local, remote, refs: [local.fullName, remote.fullName] };
  return { name: remote.short || remote.name, local: null, remote, refs: [remote.fullName] };
}

/** Remote names in the order their branches are taken for the main branch: origin first. */
function remoteOrder(refs: RefsSnapshot): string[] {
  const names = refs.remoteList.map((r) => r.name);
  return [...names.filter((n) => n === 'origin'), ...names.filter((n) => n !== 'origin')];
}

/**
 * The repository's main branch: the default branch a remote names (`origin/HEAD` first) with the local
 * branch of its name; else a local `main` / `master` with its upstream; else such a remote branch
 * (origin's first); else the checked-out branch, the first local branch, the first remote branch.
 * Null without any branch.
 */
export function mainBranch(refs: RefsSnapshot): Focus | null {
  const order = remoteOrder(refs);
  for (const name of order) {
    const head = refs.remoteList.find((r) => r.name === name)?.head;
    const remote = head ? refs.remotes.find((b) => b.fullName === head) : undefined;
    if (remote) return ofRemote(refs, remote);
  }
  for (const n of MAIN_NAMES) {
    const local = refs.locals.find((b) => b.name === n);
    if (local) return ofLocal(refs, local);
  }
  for (const n of MAIN_NAMES) {
    for (const name of order) {
      const remote = refs.remotes.find((b) => b.remote === name && b.short === n);
      if (remote) return ofRemote(refs, remote);
    }
  }
  const local = refs.locals.find((b) => b.isHead) ?? refs.locals[0];
  if (local) return ofLocal(refs, local);
  const remote = refs.remotes[0];
  return remote ? ofRemote(refs, remote) : null;
}

/** What the simple view follows: the branch `focusRef` names (local or remote) while it exists, else the main branch. */
export function focusOf(refs: RefsSnapshot | null, focusRef: string | null): Focus | null {
  if (!refs) return null;
  if (focusRef) {
    const local = refs.locals.find((b) => b.fullName === focusRef);
    if (local) return ofLocal(refs, local);
    const remote = refs.remotes.find((b) => b.fullName === focusRef);
    if (remote) return ofRemote(refs, remote);
  }
  return mainBranch(refs);
}

/** Whether two focuses walk the same refs. */
export function sameFocus(a: Focus | null, b: Focus | null): boolean {
  if (!a || !b) return a === b;
  return a.refs.length === b.refs.length && a.refs.every((r, i) => r === b.refs[i]);
}
