// Pure model of the refs sidebar: builds the (filtered) section tree from a refs snapshot and
// flattens it into fixed-height rows for VirtualList. No Svelte or DOM in here.
import type {
  LocalBranch,
  RefsSnapshot,
  Remote,
  RemoteBranch,
  Stash,
  Submodule,
  Tag,
  WorkingStatus,
  Worktree,
} from '../../types';
import { basename, shortOid } from '../../util/format';

export type SectionId = 'worktrees' | 'local' | 'remote' | 'tags' | 'stashes' | 'submodules';

/** Section names, used for tooltips only. */
export const SECTION_TITLES: Record<SectionId, string> = {
  worktrees: 'Worktrees',
  local: 'Local',
  remote: 'Remote',
  tags: 'Tags',
  stashes: 'Stashes',
  submodules: 'Submodules',
};

/** A sidebar entry backed by one ref-like object. `key` is unique and stable across refreshes. */
export type Leaf =
  | { kind: 'local'; key: string; label: string; ref: LocalBranch }
  | { kind: 'remote'; key: string; label: string; ref: RemoteBranch }
  | { kind: 'tag'; key: string; label: string; ref: Tag }
  | { kind: 'stash'; key: string; label: string; ref: Stash }
  | { kind: 'worktree'; key: string; label: string; ref: Worktree }
  | { kind: 'submodule'; key: string; label: string; ref: Submodule };

export interface Folder {
  kind: 'folder';
  /** Collapse key: `f:` + ref path of the deepest folder of a compacted chain. */
  key: string;
  /** Segment name; a compacted single-child chain joins its segments with '/'. */
  label: string;
  /** Set on the root folder of a remote. */
  remote: Remote | null;
  children: TreeNode[];
  /** Number of leaves below. */
  count: number;
  /** Full names of the hideable refs below. */
  refs: string[];
}

export type TreeNode = Folder | Leaf;

export interface Section {
  id: SectionId;
  /** Collapse key: `s:<id>`. */
  key: string;
  children: TreeNode[];
  count: number;
  /** Full names of the hideable refs in the section (local, remote, commit tags). */
  refs: string[];
}

interface RowBase {
  key: string;
  /** 0 = section header. */
  depth: number;
  /** Key of the enclosing section/folder row. */
  parent: string | null;
}

export interface SectionRow extends RowBase {
  type: 'section';
  section: Section;
  expanded: boolean;
}

export interface FolderRow extends RowBase {
  type: 'folder';
  section: SectionId;
  node: Folder;
  expanded: boolean;
}

export interface LeafRow extends RowBase {
  type: 'leaf';
  section: SectionId;
  node: Leaf;
}

export type Row = SectionRow | FolderRow | LeafRow;
export type GroupRow = SectionRow | FolderRow;

/** Collapse keys that start collapsed until the user expands them. */
const DEFAULT_COLLAPSED = new Set(['s:tags']);

export function defaultCollapsed(key: string): boolean {
  return DEFAULT_COLLAPSED.has(key);
}

// ───────────── filter ─────────────

/** Lower-cased, whitespace-separated terms of a filter query ([] = no filter). */
export function queryTerms(query: string): string[] {
  return query.trim().toLowerCase().split(/\s+/).filter(Boolean);
}

/** Every term occurs (case-insensitively) in at least one of the haystacks. */
export function matchesTerms(terms: string[], haystacks: string[]): boolean {
  const hay = haystacks.map((h) => h.toLowerCase());
  return terms.every((t) => hay.some((h) => h.includes(t)));
}

// ───────────── tree ─────────────

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/** Folders first, then natural, case-insensitive label order. */
function compareNodes(a: TreeNode, b: TreeNode): number {
  const fa = a.kind === 'folder';
  if (fa !== (b.kind === 'folder')) return fa ? -1 : 1;
  return collator.compare(a.label, b.label) || (a.label < b.label ? -1 : a.label > b.label ? 1 : 0);
}

interface Draft {
  folders: Map<string, Draft>;
  leaves: Leaf[];
}

function newDraft(): Draft {
  return { folders: new Map(), leaves: [] };
}

/** The draft folder for `name`'s parent path (created as needed) and the name's last segment. */
function place(root: Draft, name: string): [Draft, string] {
  const parts = name.split('/');
  const last = parts.pop() || name;
  let d = root;
  for (const seg of parts) {
    if (!seg) continue;
    let next = d.folders.get(seg);
    if (!next) {
      next = newDraft();
      d.folders.set(seg, next);
    }
    d = next;
  }
  return [d, last];
}

/** The tag peels to a commit. Tags on a tree or blob (e.g. linux's `v2.6.11-tree`) are never in the graph. */
export function isCommitTag(tag: Tag): boolean {
  return tag.targetType === 'commit';
}

/** Full ref name of a leaf that can be hidden / soloed in the graph (not a tag the graph never shows). */
export function hideableName(leaf: Leaf): string | null {
  if (leaf.kind === 'tag') return isCommitTag(leaf.ref) ? leaf.ref.fullName : null;
  return leaf.kind === 'local' || leaf.kind === 'remote' ? leaf.ref.fullName : null;
}

function summarize(children: TreeNode[]): { count: number; refs: string[] } {
  let count = 0;
  const refs: string[] = [];
  for (const c of children) {
    if (c.kind === 'folder') {
      count += c.count;
      for (const r of c.refs) refs.push(r);
    } else {
      count++;
      const name = hideableName(c);
      if (name) refs.push(name);
    }
  }
  return { count, refs };
}

function makeFolder(key: string, label: string, children: TreeNode[], remote: Remote | null): Folder {
  return { kind: 'folder', key, label, remote, children, ...summarize(children) };
}

/**
 * Turn a draft into sorted nodes. Single-child folder chains are compacted (`a/b/c`); a folder
 * holding exactly one leaf collapses into that leaf (label `folder/leaf`).
 */
function finish(d: Draft, path: string): TreeNode[] {
  const out: TreeNode[] = [];
  for (const [seg, child] of d.folders) {
    let label = seg;
    let p = `${path}/${seg}`;
    let c = child;
    while (c.leaves.length === 0 && c.folders.size === 1) {
      const [[s, next]] = c.folders;
      label += `/${s}`;
      p += `/${s}`;
      c = next;
    }
    if (c.folders.size === 0 && c.leaves.length === 1) {
      const leaf = c.leaves[0];
      out.push({ ...leaf, label: `${label}/${leaf.label}` });
      continue;
    }
    out.push(makeFolder(`f:${p}`, label, finish(c, p), null));
  }
  for (const leaf of d.leaves) out.push(leaf);
  return out.sort(compareNodes);
}

type Matcher = (...haystacks: string[]) => boolean;

/** One root folder per remote (configured or seen in refs); `origin:foo` matches like `origin/foo`. */
function remoteRoots(refs: RefsSnapshot, ok: Matcher): Folder[] {
  const remotes = new Map<string, Remote>();
  for (const r of refs.remoteList) remotes.set(r.name, r);
  const drafts = new Map<string, Draft>();
  const totals = new Map<string, number>();
  for (const b of refs.remotes) {
    if (!remotes.has(b.remote)) remotes.set(b.remote, { name: b.remote, url: null });
    totals.set(b.remote, (totals.get(b.remote) ?? 0) + 1);
    const short = b.short || b.name;
    if (!ok(`${b.remote}/${short}`, `${b.remote}:${short}`)) continue;
    let draft = drafts.get(b.remote);
    if (!draft) {
      draft = newDraft();
      drafts.set(b.remote, draft);
    }
    const [d, label] = place(draft, short);
    d.leaves.push({ kind: 'remote', key: b.fullName, label, ref: b });
  }
  const roots: Folder[] = [];
  for (const [name, remote] of remotes) {
    const draft = drafts.get(name);
    // A remote without fetched branches still shows (it carries the URL); one whose branches
    // all fail the filter does not.
    if (!draft && ((totals.get(name) ?? 0) > 0 || !ok(name))) continue;
    const path = `refs/remotes/${name}`;
    roots.push(makeFolder(`f:${path}`, name, draft ? finish(draft, path) : [], remote));
  }
  return roots.sort(compareNodes);
}

/** Sections in display order, filtered by `query`; empty sections are omitted. */
export function buildSections(refs: RefsSnapshot, query: string): Section[] {
  const terms = queryTerms(query);
  const ok: Matcher = (...haystacks) => terms.length === 0 || matchesTerms(terms, haystacks);
  const sections: Section[] = [];
  const add = (id: SectionId, children: TreeNode[]): void => {
    if (children.length === 0) return;
    sections.push({ id, key: `s:${id}`, children, ...summarize(children) });
  };

  // Only when linked worktrees exist; a bare repo's main entry is not a working tree.
  if (refs.worktrees.length > 1) {
    add(
      'worktrees',
      refs.worktrees
        .filter((w) => !w.bare && ok(w.path, w.branch ?? ''))
        .map((w): Leaf => ({ kind: 'worktree', key: `wt:${w.path}`, label: worktreeName(w), ref: w })),
    );
  }

  const locals = newDraft();
  for (const b of refs.locals) {
    if (!ok(b.name)) continue;
    const [d, label] = place(locals, b.name);
    d.leaves.push({ kind: 'local', key: b.fullName, label, ref: b });
  }
  add('local', finish(locals, 'refs/heads'));

  add('remote', remoteRoots(refs, ok));

  add(
    'tags',
    refs.tags.filter((t) => ok(t.name)).map((t): Leaf => ({ kind: 'tag', key: t.fullName, label: t.name, ref: t })),
  );
  add(
    'stashes',
    refs.stashes
      .filter((s) => ok(s.name, s.message))
      .map((s): Leaf => ({ kind: 'stash', key: `stash:${s.oid}`, label: stashLabel(s.message), ref: s })),
  );
  add(
    'submodules',
    refs.submodules
      .filter((m) => ok(m.path))
      .map((m): Leaf => ({ kind: 'submodule', key: `sm:${m.path}`, label: m.path, ref: m })),
  );
  return sections;
}

// ───────────── rows ─────────────

function walk(
  rows: Row[],
  nodes: TreeNode[],
  depth: number,
  parent: string,
  section: SectionId,
  collapsed: (key: string) => boolean,
): void {
  for (const n of nodes) {
    if (n.kind === 'folder') {
      const expanded = !collapsed(n.key);
      rows.push({ type: 'folder', key: n.key, depth, parent, section, node: n, expanded });
      if (expanded) walk(rows, n.children, depth + 1, n.key, section, collapsed);
    } else {
      rows.push({ type: 'leaf', key: n.key, depth, parent, section, node: n });
    }
  }
}

/** Visible rows: section headers, then the children of every expanded section/folder. */
export function flatten(sections: Section[], collapsed: (key: string) => boolean): Row[] {
  const rows: Row[] = [];
  for (const s of sections) {
    const expanded = !collapsed(s.key);
    rows.push({ type: 'section', key: s.key, depth: 0, parent: null, section: s, expanded });
    if (expanded) walk(rows, s.children, 1, s.key, s.id, collapsed);
  }
  return rows;
}

/** Row key → row index. */
export function indexRows(rows: Row[]): Map<string, number> {
  const index = new Map<string, number>();
  rows.forEach((r, i) => index.set(r.key, i));
  return index;
}

/** The outermost of `groups` that (transitively) contains the row `key`, or null. */
export function outermostAncestor(
  rows: Row[],
  index: Map<string, number>,
  key: string,
  groups: ReadonlySet<string>,
): string | null {
  let found: string | null = null;
  let i = index.get(key);
  while (i != null) {
    const parent = rows[i]?.parent;
    if (parent == null) break;
    if (groups.has(parent)) found = parent;
    i = index.get(parent);
  }
  return found;
}

/** The group row has something to expand. */
export function expandable(row: GroupRow): boolean {
  return (row.type === 'section' ? row.section.children : row.node.children).length > 0;
}

/** Hideable full ref names a row stands for (one for a leaf, all below for a group). */
export function rowRefs(row: Row): string[] {
  if (row.type === 'section') return row.section.refs;
  if (row.type === 'folder') return row.node.refs;
  const name = hideableName(row.node);
  return name ? [name] : [];
}

/** Collapse keys of every non-empty folder below `nodes`. */
export function folderKeys(nodes: TreeNode[], out: string[] = []): string[] {
  for (const n of nodes) {
    if (n.kind !== 'folder' || n.children.length === 0) continue;
    out.push(n.key);
    folderKeys(n.children, out);
  }
  return out;
}

/** Collapse keys of the folders inside a group row (not the row itself). */
export function innerFolderKeys(row: GroupRow): string[] {
  return folderKeys(row.type === 'section' ? row.section.children : row.node.children);
}

// ───────────── display helpers ─────────────

/** `↑2 ↓1`, only the non-zero parts ('' when in sync). */
export function trackLabel(ahead: number, behind: number): string {
  const parts: string[] = [];
  if (ahead > 0) parts.push(`↑${ahead}`);
  if (behind > 0) parts.push(`↓${behind}`);
  return parts.join(' ');
}

/** Stash message without git's `WIP on ` / `On ` prefix, e.g. `main: subject`. */
export function stashLabel(message: string): string {
  return message.replace(/^(?:WIP on|On) /, '') || message;
}

/** Folder name of a worktree. */
export function worktreeName(w: Worktree): string {
  const path = w.path.replace(/\/+$/, '');
  return basename(path) || w.path;
}

/**
 * What a worktree has checked out: short branch name (also the branch a rebase/bisect detached it
 * from, which the backend fills in), else short HEAD sha ('' if neither).
 */
export function worktreeDetail(w: Worktree): string {
  if (w.branch) return w.branch.replace(/^refs\/heads\//, '');
  return w.head ? shortOid(w.head) : '';
}

/** Number of distinct paths with any change (staged, unstaged, untracked or conflicted). */
export function changeCount(s: WorkingStatus): number {
  const paths = new Set<string>();
  for (const list of [s.conflicted, s.staged, s.unstaged, s.untracked]) {
    for (const e of list) paths.add(e.path);
  }
  return paths.size;
}
