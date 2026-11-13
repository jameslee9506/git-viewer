<!--
  Changed files of one diff target, as a sorted list (folder, then the name) or a folder tree
  (settings.fileListMode), each with a status icon and, at the right end, an `LFS` chip for files
  Git LFS tracks or a Binary icon for binary files (no line counts). Opening a file shows its diff
  with this list (in display order) as siblings, or calls `onopen`. Virtualized above 200 files.
  Module exports, shared by the details views: `changeSummary` (files per status, always one line),
  `fileBar` (extra · list/tree toggle · trailing, the file toolbar), `modeSeg` (list/tree toggle),
  `sectionHead` + `sectionMinHeight` (collapsible file sections: the name and its file count, a rule
  under it, an optional text button at the right: Stage All / Unstage All).
  Optional per-file `actions` (stage / unstage): a button on the hovered or cursor row's right end,
  the first entries of the row menu, Space on the cursor row.
  Files can be picked several at a time (`FileSelection`, one per details view): a click picks a file
  and opens it, ⇧-click and ⇧ with the moving keys pick the files from the anchor to the row (⇧⌘-click
  adds them), ⌘-click picks or drops one file. The actions, the menu and ⌘C on a picked row take
  every picked file.
-->
<script module lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Copy from '@lucide/svelte/icons/copy';
  import List from '@lucide/svelte/icons/list';
  import ListTree from '@lucide/svelte/icons/list-tree';
  import type { Snippet } from 'svelte';
  import { settings } from '../../state/settings.svelte';
  import StatusIcon from '../common/StatusIcon.svelte';
  import { statusClass, statusWord } from './filetree';
  import type { StatusCount } from './filetree';

  /** A lucide icon component. */
  export type Icon = typeof Copy;

  /** File row height in px (VirtualList, scrolling); the CSS uses --file-row-h, the same value. */
  const ROW_H = 28;
  /** Section header height in px, the rule under it included (`sectionHead`; the CSS of `.sec-h`). */
  const SECTION_H = 29;

  /**
   * A per-file action of a list (stage, unstage, mark resolved): `run` gets indices into `entries` —
   * the row's own, or on a row of a selection of several files every picked one, in display order.
   */
  export interface FileAction {
    icon: Icon;
    title: string;
    run: (indices: number[]) => void;
  }

  /**
   * The files picked in a details view: one selection for all of its file lists (`FileList`'s
   * `selection`; a list without one keeps its own), held by one list at a time — a pick in another
   * list starts over there.
   */
  export class FileSelection {
    /** The list holding the selection (its token), null: none. */
    owner = $state.raw<symbol | null>(null);
    /** `diffItemKey`s of the picked entries of the owner. */
    keys = $state.raw<ReadonlySet<string>>(new Set());
    /** Where ⇧-ranges start: a row key of the owner (`f\0<entry key>`, `d\0<folder path>`). */
    anchor = $state.raw<string | null>(null);

    /** The list `owner` takes the selection: `keys` picked, ranges from `anchor`. */
    set(owner: symbol, keys: ReadonlySet<string>, anchor: string | null): void {
      this.owner = owner;
      this.keys = keys;
      this.anchor = anchor;
    }

    /** A list that goes (its section closed, its last file gone) takes its selection along. */
    release(owner: symbol): void {
      if (this.owner !== owner) return;
      this.owner = null;
      this.keys = new Set();
      this.anchor = null;
    }
  }

  /** A section header's text button (Stage All, Unstage All): its label names it. */
  export interface SectionAction {
    label: string;
    run: () => void;
  }

  /** What `sectionHead` shows. */
  export interface SectionHeadData {
    title: string;
    /** Files in the section. */
    count: number;
    closed: boolean;
    /** Tinted (conflicts). */
    warn?: boolean;
    /** Button at the right end of the header. */
    action?: SectionAction;
    ontoggle: () => void;
  }

  /** CSS min-height of an expanded section of `n` files: its header (rule included), up to three rows. */
  export function sectionMinHeight(n: number): string {
    return `${SECTION_H + Math.min(n, 3) * ROW_H}px`;
  }

  /**
   * Keeps a change summary on its one line: when the line cannot hold the words they go (`terse`),
   * as the toolbar's button names do, and each status still says its word in its tooltip. Fitted
   * with the words in, whenever the line's width or its content changes.
   */
  function oneLine(line: HTMLElement): { destroy: () => void } {
    const fit = (): void => {
      line.classList.remove('terse');
      line.classList.toggle('terse', line.scrollWidth > line.clientWidth);
    };
    // Also reports the line once it is first laid out.
    const resized = new ResizeObserver(fit);
    const changed = new MutationObserver(fit);
    resized.observe(line);
    changed.observe(line, { childList: true, characterData: true, subtree: true });
    return {
      destroy() {
        resized.disconnect();
        changed.disconnect();
      },
    };
  }

  export { changeSummary, fileBar, modeSeg, sectionHead };
</script>

<script lang="ts">
  import Binary from '@lucide/svelte/icons/binary';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import RotateCcwClock from '@lucide/svelte/icons/rotate-ccw-clock';
  import ScanText from '@lucide/svelte/icons/scan-text';
  import SquareArrowOutUpRight from '@lucide/svelte/icons/square-arrow-out-up-right';
  import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener';
  import { tick, untrack } from 'svelte';
  import { errorMessage } from '../../api';
  import { activeDiffKey, getRepo } from '../../state/repo.svelte';
  import { showContextMenu, toast } from '../../state/ui.svelte';
  import type { MenuEntry } from '../../state/ui.svelte';
  import { copyText } from '../../util/format';
  import VirtualList from '../common/VirtualList.svelte';
  import {
    blameView,
    entryKeys,
    expandedTo,
    fileLabel,
    fileRows,
    fileTitle,
    historyFor,
    joinPath,
    treeView,
    visibleRows,
    worktreeCwd,
  } from './filetree';
  import type { FileEntry, FileRow } from './filetree';

  interface Props {
    entries: FileEntry[];
    /** Replaces the default open (repo.openDiff over this list in display order); index into entries. */
    onopen?: (index: number) => void;
    /** Per-file actions (working-tree lists: stage / unstage); the first one also answers Space. */
    actions?: FileAction[];
    /** The view's selection, shared with its other file lists; without one the list keeps its own. */
    selection?: FileSelection;
  }

  let { entries, onopen, actions, selection }: Props = $props();

  const VIRTUAL_MIN = 200;
  /**
   * Tree rows: indent per level (`indent`). File rows sit one level further so their status icon
   * lines up with the folder icons of their sibling folders (after the chevron). List rows start at
   * the panel's text inset (CSS).
   */
  const INDENT = 14;
  const repo = getRepo();

  const mode = $derived(settings.fileListMode);
  const keys = $derived(entryKeys(entries));
  const keyIndex = $derived(new Map(keys.map((k, i) => [k, i])));
  const allRows = $derived(fileRows(entries, mode));
  let collapsed = $state.raw<ReadonlySet<string>>(new Set());
  const rows = $derived(mode === 'tree' ? visibleRows(allRows, collapsed) : allRows);
  /** By all rows (folders included), so collapsing folders never swaps the list element. */
  const virtual = $derived(allRows.length > VIRTUAL_MIN);
  const minHeight = $derived(Math.min(rows.length, 3) * ROW_H);

  /** Row of every entry in `rows`, -1 when hidden inside a collapsed folder. */
  const rowOf = $derived.by(() => {
    const m = new Int32Array(entries.length).fill(-1);
    rows.forEach((r, i) => {
      if (r.kind === 'file') m[r.index] = i;
    });
    return m;
  });

  const activeKey = $derived(activeDiffKey(repo.view));
  const activeEntry = $derived(activeKey == null ? -1 : (keyIndex.get(activeKey) ?? -1));
  const activeRow = $derived(activeEntry < 0 ? -1 : rowOf[activeEntry]);

  /** Row of a row key (`rowKey`), -1 when no row shows it. */
  function rowIndex(k: string | null): number {
    if (k == null) return -1;
    if (k.startsWith('f\0')) {
      const e = keyIndex.get(k.slice(2));
      return e == null ? -1 : rowOf[e];
    }
    const path = k.slice(2);
    return rows.findIndex((r) => r.kind === 'dir' && r.path === path);
  }

  /** Keyboard cursor, kept by row identity so it survives refreshes and list/tree switches. */
  let cursorKey = $state<string | null>(null);
  const cursorRow = $derived(rowIndex(cursorKey));
  let focused = $state(false);

  /** This list's token as the selection's owner. */
  const me = Symbol('file list');
  const own = new FileSelection();
  const sel = $derived(selection ?? own);
  /** The selection is this list's. */
  const mine = $derived(sel.owner === me);
  /** The picked entries that show (a collapsed folder hides its files), in display order. */
  const chosen = $derived.by((): number[] => {
    const picked = mine ? sel.keys : null;
    if (!picked || picked.size === 0) return [];
    const out: number[] = [];
    for (const r of rows) if (r.kind === 'file' && picked.has(keys[r.index])) out.push(r.index);
    return out;
  });

  let root = $state.raw<HTMLDivElement>();
  let vl = $state.raw<ReturnType<typeof VirtualList>>();

  function rowKey(r: FileRow): string {
    return r.kind === 'dir' ? `d\0${r.path}` : `f\0${keys[r.index]}`;
  }

  // Follow the open diff (also when it is stepped from the viewer): move the cursor to it, pick it
  // alone (stepping through the files leaves no selection behind), reveal it.
  let followed: string | null = null;
  $effect(() => {
    const k = activeKey;
    const e = activeEntry;
    const r = activeRow;
    if (k === followed) return;
    if (k == null || e < 0) {
      followed = k;
      return;
    }
    if (r < 0) {
      untrack(() => {
        const next = expandedTo(collapsed, entries[e].item.file.path);
        if (next !== collapsed) collapsed = next;
        else followed = k;
      });
      return;
    }
    followed = k;
    untrack(() => {
      cursorKey = `f\0${k}`;
      sel.set(me, new Set([k]), cursorKey);
      scrollToRow(r);
    });
  });

  // Files that went (staged, committed, changed back) leave the selection: one that comes back later
  // comes back unpicked.
  $effect(() => {
    const index = keyIndex;
    untrack(() => {
      if (!mine) return;
      const kept = [...sel.keys].filter((k) => index.has(k));
      if (kept.length < sel.keys.size) sel.set(me, new Set(kept), sel.anchor);
    });
  });

  // A list that goes (its section closed, its last file gone) takes its selection along.
  $effect(() => {
    const s = sel;
    return () => s.release(me);
  });

  // Crossing VIRTUAL_MIN rows (e.g. a refresh adding files) swaps the list element and drops any
  // keyboard focus it held: hand that focus to the new element.
  let refocus = false;
  $effect.pre(() => {
    void virtual;
    untrack(() => {
      // Runs before the swap: the outgoing element is still `root`.
      refocus = !!root && root.contains(document.activeElement);
    });
  });
  $effect(() => {
    void virtual;
    untrack(() => {
      if (!refocus) return;
      refocus = false;
      // A removed element need not report focusout.
      focused = false;
      // Once the new element (and VirtualList) are bound; only if nothing else took the focus meanwhile.
      void tick().then(() => {
        const a = document.activeElement;
        if (!a || a === document.body) focusList();
      });
    });
  });

  // ⌘C is a native menu accelerator: it never arrives as keydown, only as clipboard events, which
  // WebKit dispatches at the selection (or body), so they are caught at the document. Cancelling
  // `beforecopy` keeps Edit ▸ Copy enabled without a text selection.
  $effect(() => {
    const owns = () => {
      const text = document.getSelection();
      if (text && !text.isCollapsed && text.toString() !== '') return false;
      return !!root && root.contains(document.activeElement);
    };
    const onBeforeCopy = (e: Event) => {
      if (owns()) e.preventDefault();
    };
    const onCopy = (e: ClipboardEvent) => {
      if (!owns() || !e.clipboardData) return;
      const row = rows[cursorRow >= 0 ? cursorRow : activeRow];
      if (!row) return;
      e.clipboardData.setData('text/plain', row.kind === 'dir' ? row.path : pathsOf(targets(row.index)));
      e.preventDefault();
      toast('Copied');
    };
    document.addEventListener('beforecopy', onBeforeCopy);
    document.addEventListener('copy', onCopy);
    return () => {
      document.removeEventListener('beforecopy', onBeforeCopy);
      document.removeEventListener('copy', onCopy);
    };
  });

  function open(index: number): void {
    if (index === activeEntry) return;
    if (onopen) {
      onopen(index);
      return;
    }
    const order: number[] = [];
    for (const r of allRows) if (r.kind === 'file') order.push(r.index);
    repo.openDiff(
      order.map((k) => entries[k].item),
      order.indexOf(index),
    );
  }

  function setCollapsed(path: string, closed: boolean): void {
    if (collapsed.has(path) === closed) return;
    const next = new Set(collapsed);
    if (closed) next.add(path);
    else next.delete(path);
    collapsed = next;
  }

  // ───────────── selection ─────────────

  /** The cursor goes to the file row `i`, and that file alone is picked: ranges start from it. */
  function pickOnly(i: number): void {
    const row = rows[i];
    if (row?.kind !== 'file') return;
    cursorKey = rowKey(row);
    sel.set(me, new Set([keys[row.index]]), cursorKey);
  }

  /** ⌘-click: the file of row `i` is picked, or left out when it was; ranges start from it. */
  function pickToggle(i: number): void {
    const row = rows[i];
    if (row?.kind !== 'file') return;
    const key = keys[row.index];
    const next = new Set(mine ? sel.keys : []);
    if (!next.delete(key)) next.add(key);
    cursorKey = rowKey(row);
    sel.set(me, next, cursorKey);
  }

  /**
   * ⇧-click, ⇧ with a moving key: the cursor goes to row `to`, and the files of the rows from the
   * anchor to it are picked, in place of the selection or added to it (`add`, ⇧⌘-click). Folder rows
   * in between pick nothing (a collapsed folder's files do not show). Without an anchor of this list
   * that shows, the range starts at row `from` (where the cursor was; a click: the row itself).
   */
  function pickRange(to: number, from: number, add: boolean): void {
    const row = rows[to];
    if (!row) return;
    let anchor = mine ? sel.anchor : null;
    let a = rowIndex(anchor);
    if (a < 0) {
      a = from >= 0 && from < rows.length ? from : to;
      anchor = rowKey(rows[a]);
    }
    const next = new Set(add && mine ? sel.keys : []);
    for (let j = Math.min(a, to); j <= Math.max(a, to); j++) {
      const r = rows[j];
      if (r.kind === 'file') next.add(keys[r.index]);
    }
    cursorKey = rowKey(row);
    sel.set(me, next, anchor);
  }

  /**
   * The entries an action on the row of entry `index` takes: every picked one when the row is one of
   * several picked files, else the row's own.
   */
  function targets(index: number): number[] {
    return chosen.length > 1 && mine && sel.keys.has(keys[index]) ? chosen : [index];
  }

  /** Repo-relative paths of entries, one per line. */
  function pathsOf(indices: number[]): string {
    return indices.map((k) => entries[k].item.file.path).join('\n');
  }

  /** Restore keyboard focus to the cursor row, else the open file's or the first row. */
  function focusList(): void {
    if (rows.length === 0) return;
    let i = cursorRow;
    if (i < 0) {
      i = activeRow >= 0 ? activeRow : 0;
      cursorKey = rowKey(rows[i]);
    }
    scrollToRow(i);
    if (virtual) vl?.focus();
    else root?.focus();
  }

  function pageSize(): number {
    if (virtual) return vl?.pageSize() ?? 1;
    return Math.max(1, Math.floor((root?.clientHeight ?? 0) / ROW_H));
  }

  function scrollToRow(i: number): void {
    if (virtual) {
      vl?.scrollToIndex(i);
      return;
    }
    const el = root;
    if (!el) return;
    const top = i * ROW_H;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_H - el.clientHeight;
  }

  /**
   * Move the cursor. A file it lands on is picked alone, and while a diff is open the diff follows;
   * a folder row leaves both as they are.
   */
  function moveTo(i: number): void {
    const row = rows[i];
    if (!row) return;
    cursorKey = rowKey(row);
    scrollToRow(i);
    if (row.kind !== 'file') return;
    pickOnly(i);
    if (repo.view.kind === 'diff') open(row.index);
  }

  /** Nearest folder row above row i that contains it (tree order), or -1. */
  function parentRow(i: number): number {
    const depth = rows[i].depth;
    for (let j = i - 1; j >= 0; j--) if (rows[j].depth < depth) return j;
    return -1;
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.isComposing || e.keyCode === 229 || rows.length === 0) return;
    const last = rows.length - 1;
    const cur = cursorRow >= 0 ? cursorRow : activeRow;
    const row = cur >= 0 ? rows[cur] : undefined;
    const bare = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
    /** The moving keys also come with ⇧: they then pick the files from the anchor on. */
    const moving = !e.metaKey && !e.ctrlKey && !e.altKey;
    const step = (d: 1 | -1) => (cur < 0 ? 0 : Math.max(0, Math.min(last, cur + d)));
    let to = -1;
    if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !e.altKey && !e.ctrlKey) {
      const down = e.key === 'ArrowDown';
      to = e.metaKey ? (down ? last : 0) : step(down ? 1 : -1);
    } else if (bare && (e.code === 'KeyJ' || e.code === 'KeyK')) {
      to = step(e.code === 'KeyJ' ? 1 : -1);
    } else if (moving && e.key === 'PageDown') {
      to = Math.min(last, Math.max(cur, 0) + pageSize());
    } else if (moving && e.key === 'PageUp') {
      to = Math.max(0, cur - pageSize());
    } else if (moving && e.key === 'Home') {
      to = 0;
    } else if (moving && e.key === 'End') {
      to = last;
    } else if (bare && e.key === 'Enter' && row) {
      if (row.kind === 'dir') setCollapsed(row.path, !collapsed.has(row.path));
      else open(row.index);
    } else if (bare && e.key === 'ArrowRight' && row) {
      if (row.kind === 'file') open(row.index);
      else if (collapsed.has(row.path)) setCollapsed(row.path, false);
      else to = Math.min(last, cur + 1);
    } else if (bare && e.key === 'ArrowLeft' && row && mode === 'tree') {
      if (row.kind === 'dir' && !collapsed.has(row.path)) setCollapsed(row.path, true);
      else {
        to = parentRow(cur);
        if (to < 0) return;
      }
    } else if (bare && e.code === 'Space' && row?.kind === 'file' && actions?.length) {
      // A held Space acts once: every action is a git run.
      if (!e.repeat) actions[0].run(targets(row.index));
    } else {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (to < 0) return;
    if (e.shiftKey) {
      pickRange(to, cur, false);
      scrollToRow(to);
    } else {
      moveTo(to);
    }
  }

  function rowAt(e: Event): number {
    const el = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-row]') : null;
    return el ? Number(el.dataset.row) : -1;
  }

  /**
   * A folder row folds or unfolds (with any modifier; the selection stays). A file row: a click picks
   * it alone and opens it, ⇧-click picks the rows from the anchor to it (⇧⌘-click adds them), ⌘-click
   * picks or drops it — neither opens anything.
   */
  function onclick(e: MouseEvent): void {
    // Ctrl-click is the macOS context click.
    if (e.ctrlKey) return;
    const i = rowAt(e);
    const row = rows[i];
    if (!row) return;
    if (row.kind === 'dir') {
      cursorKey = rowKey(row);
      setCollapsed(row.path, !collapsed.has(row.path));
      return;
    }
    // A text selection left in another pane must not capture ⌘C once files are picked.
    window.getSelection()?.removeAllRanges();
    if (e.shiftKey) {
      pickRange(i, i, e.metaKey);
    } else if (e.metaKey) {
      pickToggle(i);
    } else {
      pickOnly(i);
      open(row.index);
    }
  }

  /** The menu of a picked row is the selection's when several files are picked; any other row's is its own. */
  function oncontextmenu(e: MouseEvent): void {
    const row = rows[rowAt(e)];
    if (!row) return;
    cursorKey = rowKey(row);
    if (row.kind === 'dir') {
      showContextMenu(e, dirMenu(row.path));
      return;
    }
    const picked = targets(row.index);
    showContextMenu(e, picked.length > 1 ? selectionMenu(picked) : fileMenu(row.index));
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  function reveal(path: string): void {
    revealItemInDir(path).catch((err: unknown) => toast(errorMessage(err)));
  }

  /** Open a working-tree file (a folder: in Finder) with its default app. */
  function openFile(path: string): void {
    openPath(path).catch((err: unknown) => toast(errorMessage(err)));
  }

  function fileMenu(index: number): MenuEntry[] {
    const entry = entries[index];
    const { item } = entry;
    const { path, status } = item.file;
    const cwd = worktreeCwd(item.target);
    const blame = blameView(entry);
    const history = historyFor(item, repo.id, repo.refs);
    const tree = treeView(item);
    const menu: MenuEntry[] = [];
    if (actions?.length) {
      actions.forEach((a, k) =>
        menu.push({ label: a.title, icon: a.icon, hint: k === 0 ? 'Space' : undefined, action: () => a.run([index]) }),
      );
      menu.push({ separator: true });
    }
    menu.push({ label: 'Copy Path', icon: Copy, hint: '⌘C', action: () => copy(path) });
    if (cwd != null) menu.push({ label: 'Copy Full Path', icon: Copy, action: () => copy(joinPath(cwd, path)) });
    menu.push(
      { separator: true },
      { label: 'Blame', icon: ScanText, disabled: !blame, action: () => blame && repo.openView(blame) },
      {
        label: 'File History',
        icon: RotateCcwClock,
        disabled: !history,
        action: () => history && repo.openView(history),
      },
    );
    if (tree) menu.push({ label: 'Open at Revision', icon: FolderTree, action: () => repo.openView(tree) });
    if (cwd != null) {
      const full = joinPath(cwd, path);
      const gone = status === 'D';
      menu.push(
        { separator: true },
        { label: 'Open in Default App', icon: SquareArrowOutUpRight, disabled: gone, action: () => openFile(full) },
        { label: 'Reveal in Finder', icon: FolderOpen, disabled: gone, action: () => reveal(full) },
      );
    }
    return menu;
  }

  /**
   * The menu of several picked files (`indices`, display order): the actions, which take them all,
   * and their paths, one per line. What takes one file (blame, history, opening) is left out.
   */
  function selectionMenu(indices: number[]): MenuEntry[] {
    const note = `${indices.length} files`;
    // Taken now: a refresh while the menu is up may reorder `entries`.
    const paths = indices.map((k) => entries[k].item.file.path);
    const cwd = worktreeCwd(entries[indices[0]].item.target);
    const menu: MenuEntry[] = [];
    if (actions?.length) {
      actions.forEach((a, k) =>
        menu.push({
          label: a.title,
          note,
          icon: a.icon,
          hint: k === 0 ? 'Space' : undefined,
          action: () => a.run(indices),
        }),
      );
      menu.push({ separator: true });
    }
    menu.push({ label: 'Copy Paths', note, icon: Copy, hint: '⌘C', action: () => copy(paths.join('\n')) });
    if (cwd != null) {
      const full = paths.map((p) => joinPath(cwd, p)).join('\n');
      menu.push({ label: 'Copy Full Paths', icon: Copy, action: () => copy(full) });
    }
    return menu;
  }

  function dirMenu(path: string): MenuEntry[] {
    const menu: MenuEntry[] = [{ label: 'Copy Path', icon: Copy, hint: '⌘C', action: () => copy(path) }];
    const cwd = entries.length > 0 ? worktreeCwd(entries[0].item.target) : null;
    if (cwd != null) {
      menu.push(
        { label: 'Copy Full Path', icon: Copy, action: () => copy(joinPath(cwd, path)) },
        { separator: true },
        { label: 'Reveal in Finder', icon: FolderOpen, action: () => reveal(joinPath(cwd, path)) },
      );
    }
    return menu;
  }

  /** Left padding of a tree row `levels` in: from the panel's inset for a chevron at its edge. */
  function indent(levels: number): string {
    return `calc(var(--details-pad-btn) + ${levels * INDENT}px)`;
  }
</script>

{#snippet modeSeg()}
  <div class="seg">
    <button
      class:on={settings.fileListMode === 'list'}
      title="List"
      onclick={() => (settings.fileListMode = 'list')}
    >
      <List size={14} strokeWidth={1.75} />
    </button>
    <button
      class:on={settings.fileListMode === 'tree'}
      title="Tree"
      onclick={() => (settings.fileListMode = 'tree')}
    >
      <ListTree size={14} strokeWidth={1.75} />
    </button>
  </div>
{/snippet}

<!-- Files per status (icon · count · word; `statusCounts`) on one line: icon · count where the words
     do not fit (`oneLine`). -->
{#snippet changeSummary(counts: StatusCount[])}
  <div class="sum" use:oneLine>
    {#each counts as c (c.status)}
      <span class="sum-st" title={statusWord(c.status)}>
        <span class="st {statusClass(c.status)}"><StatusIcon status={c.status} /></span>
        <span class="sum-n">{c.count}</span>
        <span class="sum-w">{statusWord(c.status)}</span>
      </span>
    {:else}
      <span class="sum-st"><span class="sum-n">0</span><span class="sum-w">files</span></span>
    {/each}
  </div>
{/snippet}

{#snippet fileBar(extra?: Snippet, trailing?: Snippet)}
  <div class="fbar">
    {@render extra?.()}
    {@render modeSeg()}
    {@render trailing?.()}
  </div>
{/snippet}

<!-- Collapsible header of a file section (WIP conflicted / unstaged / staged; a stash's or compare's
     changes / untracked): the name with its file count right after it, `Staged (53)`, and an
     optional text button at its right end. -->
{#snippet sectionHead(h: SectionHeadData)}
  {@const action = h.action}
  <div class="sec-h" class:warn={h.warn}>
    <button class="sec-toggle" aria-expanded={!h.closed} onclick={h.ontoggle}>
      <span class="chev">
        {#if h.closed}
          <ChevronRight size={14} strokeWidth={1.75} />
        {:else}
          <ChevronDown size={14} strokeWidth={1.75} />
        {/if}
      </span>
      <span class="sec-name ellipsis">{h.title}</span>
      <span class="sec-count">({h.count})</span>
    </button>
    {#if action}
      <!-- mousedown is cancelled: the keyboard focus stays where it is -->
      <button class="sec-act" onmousedown={(e) => e.preventDefault()} onclick={action.run}>{action.label}</button>
    {/if}
  </div>
{/snippet}

{#snippet rowView(row: FileRow, i: number)}
  {#if row.kind === 'dir'}
    {@const closed = collapsed.has(row.path)}
    <div
      class="row folder"
      class:cursor={focused && i === cursorRow}
      role="treeitem"
      aria-selected="false"
      aria-expanded={!closed}
      aria-level={row.depth + 1}
      data-row={i}
      style:padding-left={indent(row.depth)}
      title={row.path}
    >
      <span class="chev">
        {#if closed}
          <ChevronRight size={14} strokeWidth={1.75} />
        {:else}
          <ChevronDown size={14} strokeWidth={1.75} />
        {/if}
      </span>
      <span class="dicon">
        {#if closed}
          <Folder size={14} strokeWidth={1.75} />
        {:else}
          <FolderOpen size={14} strokeWidth={1.75} />
        {/if}
      </span>
      <span class="name ellipsis">{row.name}</span>
    </div>
  {:else}
    {@const entry = entries[row.index]}
    {@const file = entry.item.file}
    {@const label = fileLabel(file)}
    {@const picked = mine && sel.keys.has(keys[row.index])}
    {@const many = picked && chosen.length > 1 ? chosen.length : 0}
    <div
      class="row"
      class:active={i === activeRow}
      class:picked
      class:cursor={focused && i === cursorRow}
      role="treeitem"
      aria-selected={picked}
      aria-level={row.depth + 1}
      data-row={i}
      style:padding-left={mode === 'tree' ? indent(row.depth + 1) : null}
      title={fileTitle(file)}
    >
      <span class="st {statusClass(file.status)}" title={statusWord(file.status)}>
        <StatusIcon status={file.status} />
      </span>
      {#if mode === 'tree'}
        <span class="name ellipsis">{label.name}</span>
        {#if label.fromDir != null}<span class="loc ellipsis">← {label.fromDir}</span>{/if}
      {:else}
        <!-- The folder gives way first; the name is cut only when even it does not fit. -->
        <span class="path">
          {#if label.dir}
            <span class="dir">{label.dir}</span>
            {#if !label.dir.endsWith('/')}<span class="slash">/</span>{/if}
          {/if}
          <span class="fname">{label.name}</span>
        </span>
      {/if}
      <!-- Right end: what the file is, never how many lines changed. -->
      <span class="stat">
        {#if entry.lfs}
          <span class="lfs" title="Git LFS">LFS</span>
        {:else if entry.binary}
          <span class="bin" title="Binary"><Binary size={14} strokeWidth={1.75} /></span>
        {/if}
      </span>
      {#if actions?.length}
        <!-- Shown on the hovered / cursor row; mousedown is cancelled so the focus stays put, and the
             click stops before the row's own open. On a row of several picked files (`many`) they
             take every picked file. -->
        <span class="acts">
          {#each actions as a (a.title)}
            <button
              class="act"
              title={many ? `${a.title} (${many} files)` : a.title}
              tabindex="-1"
              onmousedown={(e) => e.preventDefault()}
              onclick={(e) => {
                e.stopPropagation();
                a.run(targets(row.index));
              }}
            >
              <a.icon size={13} strokeWidth={1.75} />
            </button>
          {/each}
        </span>
      {/if}
    </div>
  {/if}
{/snippet}

{#if virtual}
  <div
    bind:this={root}
    class="fl"
    class:focused
    role="tree"
    aria-multiselectable="true"
    tabindex="-1"
    style:height="{rows.length * ROW_H}px"
    style:min-height="{minHeight}px"
    {onkeydown}
    {onclick}
    {oncontextmenu}
    onfocusin={() => (focused = true)}
    onfocusout={() => (focused = false)}
  >
    <VirtualList bind:this={vl} count={rows.length} rowHeight={ROW_H} overscan={12} tabindex={0}>
      {#snippet row(i)}
        {@render rowView(rows[i], i)}
      {/snippet}
    </VirtualList>
  </div>
{:else}
  <div
    bind:this={root}
    class="fl scroll"
    class:focused
    role="tree"
    aria-multiselectable="true"
    tabindex="0"
    style:min-height="{minHeight}px"
    {onkeydown}
    {onclick}
    {oncontextmenu}
    onfocusin={() => (focused = true)}
    onfocusout={() => (focused = false)}
  >
    {#each rows as r, i}
      {@render rowView(r, i)}
    {/each}
  </div>
{/if}

<style>
  .fl {
    position: relative;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    font-size: var(--fs-file);
  }
  .fl.scroll {
    overflow-y: auto;
  }
  /* No gap of its own: every item brings its distance to the one before or after it. Tree rows set
     their left padding (`indent`). */
  .row {
    display: flex;
    align-items: center;
    height: var(--file-row-h);
    padding: 0 calc(var(--details-pad) - 2px) 0 var(--details-pad);
    color: var(--text-0);
    white-space: nowrap;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  /* Picked files (the open one among them, unless it was left out), and the cursor a step stronger
     while the list has the focus — the graph's selected row. */
  .row.picked {
    background: var(--bg-active);
  }
  .row.cursor {
    background: var(--bg-active-strong);
  }
  .folder {
    color: var(--text-1);
  }
  .chev,
  .dicon {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
  }
  .chev {
    width: 14px;
  }
  /* A folder icon is as wide as the status cell and as close to its name: in a tree the names of a
     folder's files line up with those of its folders. */
  .dicon {
    margin-right: var(--file-icon-gap);
  }
  /* Status icon of a file row or a change summary (the global .st is a letter cell; the .st-X class
     gives the color): the panel's smaller glyph centered in the slot, the name or count close after. */
  .st {
    --status-icon-size: var(--file-status-icon-size);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--status-icon-slot);
    height: var(--status-icon-slot);
    margin-right: var(--file-icon-gap);
  }
  .sum-st > .st {
    margin-right: var(--sum-icon-gap);
  }
  .name {
    flex: 0 1 auto;
  }
  .name,
  .path,
  .loc {
    letter-spacing: var(--file-letter-spacing);
  }
  /* Tree mode "← fromDir", a step under the name: a zero basis, so it only fills what the name leaves
     (the name keeps priority). */
  .loc {
    flex: 1 1 0;
    margin-left: 6px;
    color: var(--text-2);
    font-size: var(--fs);
  }
  /*
   * List mode: dir/ name. The path takes the row's free width; inside it the folder is the only item
   * that shrinks, so it gives way before the name. The name never flexes (a flexed size can lose a
   * fraction of a pixel and show a needless "…") and is cut only when it alone is wider than the path
   * area, minus (after a folder) room for the slash (≤ 1ch) and the folder's "…" (≤ 1em): a squeezed
   * folder reads "…/", never a clipped sliver before a bare "/".
   */
  .path {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    overflow: hidden;
  }
  .dir,
  .fname {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dir {
    flex: 0 1 auto;
    min-width: 0;
    color: var(--text-2);
  }
  .slash {
    flex: none;
    color: var(--text-2);
  }
  .fname {
    flex: none;
    max-width: 100%;
  }
  .dir ~ .fname {
    max-width: calc(100% - 1ch - 1em);
  }
  /* Right end of a file row: the LFS chip or the Binary icon (muted ink: a property, not a status). */
  .stat {
    flex: none;
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    padding-left: 12px;
    color: var(--text-2);
  }
  .bin {
    display: inline-flex;
  }
  /* Outlined chip, set like the sidebar's upper-cased section labels; hue-free in both themes. */
  .lfs {
    display: inline-flex;
    align-items: center;
    height: 17px;
    padding: 0 4px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-xs);
    font-size: var(--fs-label);
    font-weight: 600;
    letter-spacing: 0.06em;
    line-height: 1;
  }
  .acts {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin-left: 8px;
    opacity: 0;
  }
  .row:hover .acts,
  .row.cursor .acts,
  .row.active .acts {
    opacity: 1;
  }
  .act {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: var(--radius-sm);
    color: var(--text-2);
  }
  .act:hover {
    background: var(--bg-active-strong);
    color: var(--text-0);
  }

  /* change summary: one line, set off from what is above it by space alone and close over the file
     toolbar. The side insets are margins: `oneLine` fits the words to the line's own box. */
  .sum {
    flex: none;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 18px;
    margin: 8px var(--details-pad) 0;
    overflow: hidden;
    font-size: var(--fs);
    line-height: 18px;
    white-space: nowrap;
  }
  .sum-st {
    flex: none;
    display: inline-flex;
    align-items: center;
  }
  .sum-n {
    color: var(--text-0);
    font-variant-numeric: tabular-nums;
  }
  .sum-w {
    margin-left: 5px;
    color: var(--sum-word-fg);
  }
  /* Set by `oneLine`, outside Svelte's view of the markup. */
  .sum:global(.terse) .sum-w {
    display: none;
  }

  /* file toolbar */
  .fbar {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    height: var(--toolbar-h);
    padding: 0 var(--details-pad-btn) 0 var(--details-pad);
    border-bottom: 1px solid var(--border);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .seg {
    flex: none;
  }
  .seg > button {
    height: 22px;
    min-width: 28px;
  }
  /* section header: on the panel's own background with a rule under it — a section's only rule (none
     closes a section: the next header follows its rows); the toggle fills it, a text button (when
     any) sits at its right end */
  .sec-h {
    flex: none;
    display: flex;
    align-items: center;
    width: 100%;
    height: 29px; /* 28px and the rule: SECTION_H */
    border-bottom: 1px solid var(--border);
    color: var(--text-1);
  }
  .sec-h.warn {
    color: var(--st-conflict);
  }
  .sec-toggle {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    min-width: 0;
    height: 100%;
    padding: 0 var(--details-pad) 0 var(--details-pad-btn);
    color: inherit;
    font-size: var(--fs);
    text-align: left;
    transition: color var(--dur);
  }
  .sec-toggle .chev {
    transition: color var(--dur);
  }
  /* Under the pointer the ink steps up, with no fill; a conflicted header keeps its hue. */
  .sec-toggle:hover {
    color: var(--text-0);
  }
  .sec-h.warn .sec-toggle {
    color: inherit;
  }
  .sec-toggle:hover .chev {
    color: var(--text-1);
  }
  /* The name starts where a list row's file name does: the chevron's 14px and 6px after it. */
  .sec-name {
    margin-left: 6px;
    font-weight: 600;
  }
  /* `(53)`: right after the name, a space's width on, in its ink at the regular weight */
  .sec-count {
    flex: none;
    margin-left: 4px;
    font-variant-numeric: tabular-nums;
  }
  /* Stage All / Unstage All: an ink outline as wide as its label, its box on the panel's inset */
  .sec-act {
    flex: none;
    display: inline-flex;
    align-items: center;
    height: 20px;
    margin-right: var(--details-pad);
    padding: 0 6px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--text-1);
    font-size: var(--fs-sm);
    font-weight: 600;
    white-space: nowrap;
    transition:
      background var(--dur),
      color var(--dur);
  }
  .sec-act:hover {
    background: var(--bg-3);
    color: var(--text-0);
  }
</style>
