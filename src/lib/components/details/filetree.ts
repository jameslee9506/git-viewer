// Pure helpers for the details file lists: entries, status counts, list/tree rows, labels
// and per-file view targets.
import { diffItemKey } from '../../state/repo.svelte';
import type { CenterView, DiffItem, FileRef } from '../../state/repo.svelte';
import type { ChangeStatus, DiffTarget, FileChange, RefsSnapshot, StatusEntry } from '../../types';

/** One file of a FileList. */
export interface FileEntry {
  item: DiffItem;
  additions: number | null;
  deletions: number | null;
  binary: boolean;
  /** A submodule (gitlink) entry: a commit pointer, no file content to blame. */
  submodule: boolean;
}

export type FileListMode = 'list' | 'tree';

/** A rendered row: a folder (tree mode; a compacted single-child chain) or a file (index into entries). */
export type FileRow =
  | { kind: 'dir'; path: string; name: string; depth: number }
  | { kind: 'file'; index: number; depth: number };

export function entriesFromChanges(target: DiffTarget, files: FileChange[]): FileEntry[] {
  return files.map((f) => ({
    item: { target, file: { path: f.path, oldPath: f.oldPath, status: f.status } },
    additions: f.additions,
    deletions: f.deletions,
    binary: f.binary,
    submodule: f.submodule,
  }));
}

export function entriesFromStatus(target: DiffTarget, list: StatusEntry[]): FileEntry[] {
  return list.map((s) => ({
    item: { target, file: { path: s.path, oldPath: s.oldPath, status: s.status } },
    additions: null,
    deletions: null,
    binary: false,
    submodule: s.submodule,
  }));
}

/** Sums of the known line counts. */
export function lineTotals(files: readonly { additions: number | null; deletions: number | null }[]): {
  additions: number;
  deletions: number;
} {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    additions += f.additions ?? 0;
    deletions += f.deletions ?? 0;
  }
  return { additions, deletions };
}

/** Order of the statuses in a change summary. */
export const STATUS_ORDER: readonly ChangeStatus[] = ['M', 'A', 'D', 'R', 'C', 'T', 'U', '?'];

/** Each status in words: the change summary and the status icon tooltips. */
export const STATUS_WORDS: Readonly<Record<ChangeStatus, string>> = {
  M: 'modified',
  A: 'added',
  D: 'deleted',
  R: 'renamed',
  C: 'copied',
  T: 'type changed',
  U: 'conflicted',
  '?': 'untracked',
};

export interface StatusCount {
  status: ChangeStatus;
  count: number;
}

/** `STATUS_WORDS`; the letter itself for one git reports outside `ChangeStatus` (its `X`). */
export function statusWord(status: ChangeStatus): string {
  return STATUS_WORDS[status] ?? status;
}

/**
 * Number of files per status in `STATUS_ORDER`, statuses without files left out. Letters outside
 * `ChangeStatus` come last, so the counts always add up to the number of files.
 */
export function statusCounts(files: readonly { status: ChangeStatus }[]): StatusCount[] {
  const n = new Map<ChangeStatus, number>();
  for (const f of files) n.set(f.status, (n.get(f.status) ?? 0) + 1);
  const out: StatusCount[] = [];
  for (const status of STATUS_ORDER) {
    const count = n.get(status);
    if (count) out.push({ status, count });
    n.delete(status);
  }
  for (const [status, count] of n) out.push({ status, count });
  return out;
}

/** `diffItemKey` of every entry (the same key `activeDiffKey` gives the open diff). */
export function entryKeys(entries: readonly FileEntry[]): string[] {
  return entries.map((e) => diffItemKey(e.item));
}

/** Path segments; an untracked nested repository (`dir/sub/`) keeps its slash on the last segment. */
function segments(path: string): string[] {
  if (!path.endsWith('/')) return path.split('/');
  const parts = path.slice(0, -1).split('/');
  parts[parts.length - 1] += '/';
  return parts;
}

function splitDirBase(path: string): [dir: string, base: string] {
  const segs = segments(path);
  const base = segs.pop() ?? '';
  return [segs.join('/'), base];
}

function compareKeys(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** List mode: every file at depth 0, sorted by path (case-insensitive, stable). */
export function listRows(entries: readonly FileEntry[]): FileRow[] {
  const keys = entries.map((e) => e.item.file.path.toLowerCase());
  const order = entries.map((_, i) => i);
  order.sort((a, b) => compareKeys(keys[a], keys[b]) || a - b);
  return order.map((index) => ({ kind: 'file', index, depth: 0 }));
}

interface DirNode {
  name: string;
  path: string;
  dirs: Map<string, DirNode>;
  files: number[];
}

/** Tree mode, fully expanded: folders first, then files, each by name; single-child folder chains compacted. */
export function treeRows(entries: readonly FileEntry[]): FileRow[] {
  const root: DirNode = { name: '', path: '', dirs: new Map(), files: [] };
  const names: string[] = [];
  entries.forEach((e, i) => {
    const segs = segments(e.item.file.path);
    names.push(segs[segs.length - 1].toLowerCase());
    let node = root;
    for (let k = 0; k < segs.length - 1; k++) {
      const name = segs[k];
      let next = node.dirs.get(name);
      if (!next) {
        next = { name, path: node.path ? `${node.path}/${name}` : name, dirs: new Map(), files: [] };
        node.dirs.set(name, next);
      }
      node = next;
    }
    node.files.push(i);
  });
  const out: FileRow[] = [];
  walk(root, 0, names, out);
  return out;
}

function walk(node: DirNode, depth: number, names: string[], out: FileRow[]): void {
  const dirs = [...node.dirs.values()].sort(
    (a, b) => compareKeys(a.name.toLowerCase(), b.name.toLowerCase()) || compareKeys(a.name, b.name),
  );
  for (const start of dirs) {
    let dir = start;
    let name = dir.name;
    while (dir.files.length === 0 && dir.dirs.size === 1) {
      const [only] = dir.dirs.values();
      name += `/${only.name}`;
      dir = only;
    }
    out.push({ kind: 'dir', path: dir.path, name, depth });
    walk(dir, depth + 1, names, out);
  }
  const files = [...node.files].sort((a, b) => compareKeys(names[a], names[b]) || a - b);
  for (const index of files) out.push({ kind: 'file', index, depth });
}

export function fileRows(entries: readonly FileEntry[], mode: FileListMode): FileRow[] {
  return mode === 'tree' ? treeRows(entries) : listRows(entries);
}

/** Rows left after hiding the descendants of collapsed folders (rows must be in tree order). */
export function visibleRows(rows: FileRow[], collapsed: ReadonlySet<string>): FileRow[] {
  if (collapsed.size === 0) return rows;
  const out: FileRow[] = [];
  let hideBelow = -1;
  for (const r of rows) {
    if (hideBelow >= 0) {
      if (r.depth > hideBelow) continue;
      hideBelow = -1;
    }
    out.push(r);
    if (r.kind === 'dir' && collapsed.has(r.path)) hideBelow = r.depth;
  }
  return out;
}

/** Entry indices in display order (collapsed folders included), used as the diff sibling order. */
export function displayOrder(entries: readonly FileEntry[], mode: FileListMode): number[] {
  const out: number[] = [];
  for (const r of fileRows(entries, mode)) if (r.kind === 'file') out.push(r.index);
  return out;
}

/** Collapsed folders minus every ancestor of `path`. Returns the same set when nothing changes. */
export function expandedTo(collapsed: ReadonlySet<string>, path: string): ReadonlySet<string> {
  let next: Set<string> | null = null;
  for (const dir of collapsed) {
    if (path.startsWith(`${dir}/`)) {
      next ??= new Set(collapsed);
      next.delete(dir);
    }
  }
  return next ?? collapsed;
}

export interface FileLabel {
  /** Base name; `old → new` when a rename changed it. */
  name: string;
  /** Folder; `old → new` when the file moved (root shown as `/`). */
  dir: string;
  /** Previous folder when the file moved (tree mode shows it next to the name). */
  fromDir: string | null;
}

export function fileLabel(file: FileRef): FileLabel {
  const [dir, base] = splitDirBase(file.path);
  if (file.oldPath == null || file.oldPath === file.path) return { name: base, dir, fromDir: null };
  const [oldDir, oldBase] = splitDirBase(file.oldPath);
  const moved = oldDir !== dir;
  return {
    name: oldBase === base ? base : `${oldBase} → ${base}`,
    dir: moved ? `${oldDir || '/'} → ${dir || '/'}` : dir,
    fromDir: moved ? oldDir || '/' : null,
  };
}

/** Row tooltip: the full path, `old → new` for renames and copies. */
export function fileTitle(file: FileRef): string {
  return file.oldPath != null && file.oldPath !== file.path ? `${file.oldPath} → ${file.path}` : file.path;
}

/** Status color class (`.st st-X`; untracked `?` → `st-q`). */
export function statusClass(status: ChangeStatus): string {
  return status === '?' ? 'st-q' : `st-${status}`;
}

/** Worktree directory of a working-tree target, null for targets addressed by oids. */
export function worktreeCwd(target: DiffTarget): string | null {
  switch (target.kind) {
    case 'staged':
    case 'unstaged':
    case 'untracked':
    case 'conflicted':
    case 'worktree':
      return target.cwd;
    default:
      return null;
  }
}

/** Absolute path of a repo-relative path inside a worktree (a nested repo's trailing slash dropped). */
export function joinPath(cwd: string, path: string): string {
  return `${cwd.replace(/\/+$/, '')}/${path.replace(/\/+$/, '')}`;
}

/** Revisions holding the new and old side of a file for oid-addressed targets. */
function sides(target: DiffTarget): { newRev: string; oldRev: string | null } | null {
  switch (target.kind) {
    case 'commit':
      return { newRev: target.oid, oldRev: target.parent ?? `${target.oid}^` };
    case 'range':
      return { newRev: target.to, oldRev: target.from };
    case 'stash':
      return { newRev: target.oid, oldRev: `${target.oid}^1` };
    case 'stashUntracked':
      return { newRev: `${target.oid}^3`, oldRev: null };
    default:
      return null;
  }
}

/**
 * Blame of a listed file: at the commit (deleted files: at the old side), or the working tree file
 * (deleted: at the worktree's HEAD). Null when there is nothing to blame: binary files, submodules
 * (git blame fails on a gitlink) and untracked files.
 */
export function blameView(entry: FileEntry): CenterView | null {
  if (entry.binary || entry.submodule) return null;
  const { target, file } = entry.item;
  const cwd = worktreeCwd(target);
  if (cwd != null) {
    if (file.status === '?') return null;
    // Deleted from disk: blame the version on the other side (the compared commit, else HEAD).
    if (file.status === 'D') {
      return { kind: 'blame', path: file.oldPath ?? file.path, rev: target.kind === 'worktree' ? target.oid : 'HEAD', cwd };
    }
    return { kind: 'blame', path: file.path, rev: null, cwd };
  }
  const s = sides(target);
  if (!s || target.kind === 'stashUntracked') return null;
  if (file.status === 'D') return s.oldRev ? { kind: 'blame', path: file.path, rev: s.oldRev, cwd: null } : null;
  return { kind: 'blame', path: file.path, rev: s.newRev, cwd: null };
}

/**
 * History of a listed file, starting at the commit it is listed for (range: the side that has it;
 * stash: its base). Working-tree files start at that worktree's HEAD (`head`, null = the opened one)
 * under their HEAD path. Null for files without history (untracked, added in a stash).
 */
export function historyView(item: DiffItem, head: string | null): CenterView | null {
  const { target, file } = item;
  switch (target.kind) {
    case 'commit':
      return { kind: 'history', path: file.path, rev: target.oid };
    case 'range':
      return { kind: 'history', path: file.path, rev: file.status === 'D' ? target.from : target.to };
    case 'stash':
      if (file.status === 'A' || file.status === '?') return null;
      return { kind: 'history', path: file.oldPath ?? file.path, rev: `${target.oid}^1` };
    case 'stashUntracked':
      return null;
    default:
      if (file.status === '?') return null;
      return { kind: 'history', path: file.oldPath ?? file.path, rev: head };
  }
}

/**
 * HEAD of the worktree a working-tree target belongs to (where its file history starts); null for
 * the opened worktree (history then starts at its HEAD) and for targets addressed by oids.
 */
export function targetHead(target: DiffTarget, repoId: string, refs: RefsSnapshot | null): string | null {
  const cwd = worktreeCwd(target);
  if (cwd == null || cwd === repoId) return null;
  return refs?.worktrees.find((w) => w.path === cwd)?.head ?? null;
}

/** `historyView` of a file listed in the repo `repoId`: the one rule for every History entry point. */
export function historyFor(item: DiffItem, repoId: string, refs: RefsSnapshot | null): CenterView | null {
  return historyView(item, targetHead(item.target, repoId, refs));
}

/** The file in the tree browser at the revision that contains it (deleted: the old side). */
export function treeView(item: DiffItem): CenterView | null {
  const { target, file } = item;
  const s = sides(target);
  if (!s) return null;
  if (file.status === 'D') return s.oldRev ? { kind: 'tree', rev: s.oldRev, path: file.path } : null;
  return { kind: 'tree', rev: s.newRev, path: file.path };
}
