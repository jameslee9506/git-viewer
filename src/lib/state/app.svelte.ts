// App-level state: the tab bar's items (tabs and groups of tabs), the open repositories behind them,
// what the page area shows, recent and recently closed repos, clones, session restore.
import { ask, open as openDialog } from '@tauri-apps/plugin-dialog';
import { api, errorMessage } from '../api';
import { basename, dirname } from '../util/format';
import { forgetRepo } from './cache';
import { loadJSON, saveJSON } from './persist';
import { RepoStore } from './repo.svelte';
import { toast } from './ui.svelte';

export interface Tab {
  id: number;
  /** null = a new tab: the open screen. Tabs of one repository share its store. */
  repo: RepoStore | null;
}

/** A named group of tabs. Groups sit at the top level of the bar only: a group holds no group. */
export interface Group {
  id: number;
  name: string;
  tabs: Tab[];
}

export type Item = Tab | Group;

export function isGroup(item: Item): item is Group {
  return 'tabs' in item;
}

/**
 * A place in the bar: position `index` among the tabs of the group `group`, or among the top-level
 * items when `group` is null.
 */
export interface Place {
  group: number | null;
  index: number;
}

/** What the page area holds: one page per open repository (its tabs share it) and one per new tab. */
export interface Page {
  key: number;
  repo: RepoStore | null;
}

export interface RecentRepo {
  path: string;
  name: string;
  /** unix ms */
  openedAt: number;
}

/** A repository whose last tab the user closed, and that has not opened again since. */
export interface ClosedRepo {
  path: string;
  name: string;
  /** unix ms */
  closedAt: number;
}

/** A clone running for a new tab: the repository opens in that tab once it is there. */
export interface Clone {
  tab: number;
  url: string;
  /** The folder it clones into (absolute). */
  dest: string;
}

interface OpenOptions {
  /** Always a tab of its own: the active new tab is not reused. */
  newTab?: boolean;
  /** The group it goes into (null: the top level) instead of where the user is. */
  group?: number | null;
  /** Leave what shows alone. */
  background?: boolean;
  /** The new tab it fills (a clone's), wherever that tab is by then; nothing opens once it is gone. */
  into?: number;
}

/** The bar as stored: a repository path, or a group with the paths of its tabs. */
type Saved = string | { group: string; tabs: string[] };

function isSavedGroup(s: unknown): s is { group: string; tabs: string[] } {
  if (typeof s !== 'object' || s === null) return false;
  const g = s as { group?: unknown; tabs?: unknown };
  return typeof g.group === 'string' && Array.isArray(g.tabs);
}

/** A stored list of repositories: its entries that have a path and a name, one per path. */
function loadRepos<T extends { path: string; name: string }>(key: string): T[] {
  const stored = loadJSON<unknown>(key, []);
  const seen = new Set<string>();
  return (Array.isArray(stored) ? stored : []).filter((r): r is T => {
    const e = r as { path?: unknown; name?: unknown } | null;
    if (typeof e?.path !== 'string' || typeof e.name !== 'string' || seen.has(e.path)) return false;
    seen.add(e.path);
    return true;
  });
}

const MAX_RECENT = 20;
const MAX_CLOSED = 5;
const NEW_GROUP_NAME = 'New Group';

class AppState {
  /** The bar: tabs and groups in order. Replaced, never mutated — as are the groups in it. */
  items = $state.raw<Item[]>([]);
  /** The tab that shows. Null while there is none to show: the open screen, without a tab. */
  activeId = $state<number | null>(null);
  /**
   * The group whose tabs the bar shows in place of the top level: one that was picked. Null while
   * the bar shows the top level. Which row the bar shows and which tab shows are apart: picking a
   * group changes the row and leaves the page alone, so the active tab may be in another row.
   */
  openGroupId = $state<number | null>(null);
  /** The stored session is open (`restore` is through): until then, what shows is still to come. */
  restored = $state(false);
  /** The group whose name is being typed in the bar. */
  renaming = $state<number | null>(null);
  recent = $state.raw<RecentRepo[]>(loadRepos<RecentRepo>('recent'));
  /** Most recent first; a repository leaves it when it opens again. */
  closed = $state.raw<ClosedRepo[]>(loadRepos<ClosedRepo>('closed').slice(0, MAX_CLOSED));
  clones = $state.raw<Clone[]>([]);
  /** Path currently being opened (welcome screen spinner). */
  opening = $state<string | null>(null);
  /** The folder `createWithDialog` makes a repository of (welcome screen spinner). */
  creating = $state<string | null>(null);
  error = $state<string | null>(null);

  /** Ids of tabs, groups and pages: one counter, so the pages keep their creation order. */
  #nextId = 1;
  /** The open repositories by path. A repository shown by several tabs has one store. */
  #stores = new Map<string, RepoStore>();
  #pageKeys = new WeakMap<RepoStore, number>();
  /** The tab each group showed last, by group id. */
  #lastIn = new Map<number, number>();
  /**
   * What `restore` has yet to open of the stored session: the entries not begun, and the paths left
   * of the group it is filling. They are stored along with the bar until they are open, so the
   * session is whole in storage whenever the app quits (or an open never returns).
   */
  #pending: Saved[] = [];
  #filling: { id: number; paths: string[] } | null = null;
  /** The bar was stored since the app started. */
  #stored = false;

  /** Every tab in bar order, a group's in its place. */
  tabs = $derived(this.items.flatMap((it) => (isGroup(it) ? it.tabs : [it])));
  active = $derived(this.tabs.find((t) => t.id === this.activeId) ?? null);
  openGroup = $derived(this.items.find((it): it is Group => isGroup(it) && it.id === this.openGroupId) ?? null);
  /** The group that holds the active tab; null for a tab at the top level, and without one. */
  activeGroup = $derived(this.activeId == null ? null : this.groupOf(this.activeId));
  /**
   * The row the bar shows: the open group's tabs, else the top-level items. ⌘1…⌘9 count its cells,
   * ⌃Tab and ⌥⌘← / ⌥⌘→ step through its tabs.
   */
  stops = $derived<readonly Item[]>(this.openGroup ? this.openGroup.tabs : this.items);
  /** The pages in creation order (keys only grow): the bar's order must never move a page's DOM. */
  pages = $derived.by(() => {
    const out: Page[] = [];
    const seen = new Set<RepoStore>();
    for (const t of this.tabs) {
      if (!t.repo) {
        out.push({ key: t.id, repo: null });
      } else if (!seen.has(t.repo)) {
        seen.add(t.repo);
        out.push({ key: this.#pageKeys.get(t.repo) ?? 0, repo: t.repo });
      }
    }
    return out.sort((a, b) => a.key - b.key);
  });
  /** The page of the active tab; null without one. */
  activePage = $derived.by(() => {
    const t = this.active;
    if (!t) return null;
    return t.repo ? (this.#pageKeys.get(t.repo) ?? null) : t.id;
  });

  // ───────────── lookups ─────────────

  #group(id: number | null): Group | null {
    if (id == null) return null;
    return this.items.find((it): it is Group => isGroup(it) && it.id === id) ?? null;
  }

  #tab(id: number): Tab | null {
    return this.tabs.find((t) => t.id === id) ?? null;
  }

  /** The tabs beside each other in `group`, or at the top level. */
  #row(group: Group | null): Tab[] {
    return group ? group.tabs : this.items.filter((it): it is Tab => !isGroup(it));
  }

  #setTabs(id: number, tabs: Tab[]): void {
    this.items = this.items.map((it) => (isGroup(it) && it.id === id ? { ...it, tabs } : it));
  }

  /** The group that holds the tab `id`; null for a tab at the top level. */
  groupOf(id: number): Group | null {
    for (const it of this.items) if (isGroup(it) && it.tabs.some((t) => t.id === id)) return it;
    return null;
  }

  /** Where the tab or group `id` is. */
  placeOf(id: number): Place | null {
    for (let i = 0; i < this.items.length; i++) {
      const it = this.items[i];
      if (it.id === id) return { group: null, index: i };
      if (!isGroup(it)) continue;
      const k = it.tabs.findIndex((t) => t.id === id);
      if (k >= 0) return { group: it.id, index: k };
    }
    return null;
  }

  // ───────────── opening ─────────────

  /**
   * Open a repo (any path inside it) where the user is: in the new tab that shows — it is filled,
   * in whichever row it is —, else in the row the bar shows (the open group, else the top level).
   * A tab of that repository already in that row is focused instead. One in another row stays, and
   * the new tab shows the same repository through the same store. The bar keeps its row.
   */
  async open(path: string, opts: OpenOptions = {}): Promise<void> {
    // A clone's tab shows a spinner of its own.
    if (opts.into == null) this.opening = path;
    this.error = null;
    try {
      const info = await api.openRepo(path);
      // Looked up now, not before the wait: the user may have moved on.
      const into = opts.into == null ? null : this.#tab(opts.into);
      const cur = this.active;
      /** The active new tab, when it is to be filled: no tab of its own was asked for, and no clone runs for it. */
      const fill = cur && cur.repo === null && !opts.newTab && !this.cloneFor(cur.id) ? cur : null;
      const group =
        opts.into != null
          ? into && this.groupOf(into.id)
          : opts.group !== undefined
            ? this.#group(opts.group)
            : fill
              ? this.groupOf(fill.id)
              : this.openGroup;
      const held = this.#stores.get(info.path);
      // The group it was meant for was deleted meanwhile, or the new tab it was to fill closed or
      // shows a repository by now: nowhere to go.
      const lost = opts.into != null && (into === null || into.repo !== null);
      const gone = (opts.group != null && !group) || lost;
      // openRepo took a backend reference; a repository that is open holds its own already.
      if (held || gone) void api.closeRepo(info.path).catch(() => {});
      if (gone) return;
      const row: Item[] = group ? group.tabs : this.items;
      const existing = row.find((it) => !isGroup(it) && it.repo?.id === info.path);
      if (existing) {
        if (!opts.background && !isGroup(existing)) this.#show(existing);
        return;
      }
      const repo = held ?? this.#adopt(new RepoStore(info));
      // A clone's repository fills the clone's tab. Otherwise the active new tab of that row is reused
      // (same id) unless a tab of its own was asked for, or a clone runs for it: that one fills it.
      const reuse = into ?? (fill && row.includes(fill) ? fill : null);
      const tab: Tab = { id: reuse ? reuse.id : this.#nextId++, repo };
      const others = reuse ? row.filter((it) => it !== reuse) : row;
      // Worktrees of one repository share commonDir (canonical, also for bare repos): keep their
      // tabs adjacent by inserting after the last of them (features-ux M14). Without one a reused
      // new tab keeps its slot and a new tab goes last. Reverse loop: lib is ES2022.
      let at = reuse ? row.indexOf(reuse) : others.length;
      for (let i = others.length - 1; i >= 0; i--) {
        const o = others[i];
        if (!isGroup(o) && o.repo?.info.commonDir === info.commonDir) {
          at = i + 1;
          break;
        }
      }
      const next = [...others.slice(0, at), tab, ...others.slice(at)];
      if (group) this.#setTabs(group.id, next as Tab[]);
      else this.items = next;
      // Also in the background when it is the first tab of the open group while no tab shows.
      const first = group != null && group.id === this.openGroupId && this.activeId == null;
      if (!opts.background || first) this.#show(tab);
      this.#remember(info.path, info.name);
      this.#persist();
      if (!held) void repo.init();
    } catch (e) {
      this.error = errorMessage(e);
      toast(this.error);
    } finally {
      if (this.opening === path) this.opening = null;
    }
  }

  /** A repository opened for the first time: its store, and the key of the page that shows it. */
  #adopt(repo: RepoStore): RepoStore {
    this.#stores.set(repo.id, repo);
    this.#pageKeys.set(repo, this.#nextId++);
    return repo;
  }

  /**
   * Close the repositories no tab shows any more: they head the recently closed. Stop the clones
   * whose tab is gone.
   */
  #release(): void {
    const shown = new Set(this.tabs.map((t) => t.repo));
    const closed: ClosedRepo[] = [];
    for (const [path, repo] of this.#stores) {
      if (shown.has(repo)) continue;
      this.#stores.delete(path);
      repo.dispose();
      forgetRepo(path);
      closed.push({ path, name: repo.info.name, closedAt: Date.now() });
    }
    if (closed.length > 0) this.#setClosed([...closed, ...this.closed]);
    for (const c of this.clones) if (!this.#tab(c.tab)) this.cancelClone(c.tab);
  }

  /** Where the Finder panels start: the folder holding the active repo, or the last opened one. */
  #near(): string | undefined {
    const near = this.active?.repo?.id ?? this.recent[0]?.path;
    return near ? dirname(near) || '/' : undefined;
  }

  /** Finder folder panel; each picked folder opens like `open` (one tab per folder, like a drop). */
  async openWithDialog(opts: { newTab?: boolean } = {}): Promise<void> {
    const picked = await openDialog({
      directory: true,
      multiple: true,
      canCreateDirectories: false,
      defaultPath: this.#near(),
    });
    for (const path of picked ?? []) await this.open(path, { newTab: opts.newTab });
  }

  /**
   * Finder folder panel with a New Folder button: `git init` in the folder picked, which then opens
   * like `open`. A folder that holds anything asks first: the panel's Open with no folder picked
   * hands over the folder it shows. One that holds a repository already just opens. Rejects with
   * git's message.
   */
  async createWithDialog(): Promise<void> {
    const path = await openDialog({
      directory: true,
      multiple: false,
      canCreateDirectories: true,
      defaultPath: this.#near(),
    });
    if (!path) return;
    this.creating = path;
    try {
      if (!(await api.initRepo(path, false))) {
        let ok = false;
        try {
          ok = await ask(`Create a repository in “${basename(path)}”? It is not empty.`, {
            kind: 'warning',
            okLabel: 'Create',
            cancelLabel: 'Cancel',
          });
        } catch {
          return; // not running inside Tauri
        }
        if (!ok) return;
        await api.initRepo(path, true);
      }
      await this.open(path);
    } finally {
      if (this.creating === path) this.creating = null;
    }
  }

  // ───────────── cloning ─────────────

  /** The clone running for the new tab `id`. */
  cloneFor(id: number | null): Clone | null {
    return id == null ? null : (this.clones.find((c) => c.tab === id) ?? null);
  }

  /**
   * Clone `url` into the folder `dest` for the new tab `tab` — from the open screen of a group
   * without tabs (null), for a new tab of that group. The repository opens in that tab once it is
   * there, wherever the user went meanwhile; closing the tab stops the clone.
   */
  async clone(url: string, dest: string, tab: number | null): Promise<void> {
    // One clone into a folder at a time: the backend refuses a second one too, and a Cancel, which
    // names the folder, would stop the first.
    if (this.clones.some((c) => c.dest === dest)) {
      this.error = `already cloning into ${dest}`;
      toast(this.error);
      return;
    }
    if (tab == null) this.newTab();
    const id = tab ?? this.activeId;
    if (id == null || this.cloneFor(id)) return;
    const job: Clone = { tab: id, url, dest };
    this.clones = [...this.clones, job];
    this.error = null;
    try {
      await api.cloneRepo(url, dest);
    } catch (e) {
      // Stopped (Cancel, its tab closed): nothing to tell.
      if (!this.clones.includes(job)) return;
      this.clones = this.clones.filter((c) => c !== job);
      this.error = errorMessage(e);
      toast(this.error);
      return;
    }
    // Until it is open, the tab shows the clone.
    if (this.clones.includes(job)) await this.open(dest, { into: id, background: true });
    this.clones = this.clones.filter((c) => c !== job);
  }

  /** Stop the clone running for the tab `id`: git removes what it made of the folder. */
  cancelClone(id: number): void {
    const job = this.cloneFor(id);
    if (!job) return;
    this.clones = this.clones.filter((c) => c !== job);
    void api.cancelClone(job.dest).catch(() => {});
  }

  /** A new tab where the user is: last in the open group, else last at the top level. */
  newTab(): void {
    const tab: Tab = { id: this.#nextId++, repo: null };
    const group = this.openGroup;
    if (group) this.#setTabs(group.id, [...group.tabs, tab]);
    else this.items = [...this.items, tab];
    this.activate(tab.id);
  }

  // ───────────── what shows ─────────────

  /** Show the tab `tab`. The bar keeps its row. */
  #show(tab: Tab): void {
    this.activeId = tab.id;
    const group = this.groupOf(tab.id);
    if (group) this.#lastIn.set(group.id, tab.id);
  }

  /** The tab `group` showed last, else its first. */
  #lastOf(group: Group): Tab | undefined {
    const last = this.#lastIn.get(group.id);
    return group.tabs.find((t) => t.id === last) ?? group.tabs[0];
  }

  /** Show a tab, and its row in the bar: its group's, or the top level. */
  activate(id: number): void {
    const tab = this.#tab(id);
    if (!tab) return;
    this.#show(tab);
    this.openGroupId = this.groupOf(id)?.id ?? null;
  }

  /** Open a group: the bar shows its tabs in place of the top level. What shows stays. */
  enter(id: number): void {
    if (this.#group(id)) this.openGroupId = id;
  }

  /** Leave the open group: the bar shows the top level again. What shows stays. */
  leave(): void {
    this.openGroupId = null;
  }

  /** Into the group of the tab that shows, from the top level. */
  enterActive(): void {
    const group = this.activeGroup;
    if (group && this.openGroupId == null) this.enter(group.id);
  }

  /** Go to a cell of the row the bar shows by 0-based position (Cmd+1..9; 9 = the last): a group is opened. */
  activateIndex(i: number): void {
    const stops = this.stops;
    const stop = i >= 8 ? stops[stops.length - 1] : stops[i];
    if (!stop) return;
    if (isGroup(stop)) this.enter(stop.id);
    else this.activate(stop.id);
  }

  /**
   * The next (or the previous) tab of the row the bar shows, around its ends. At the top level the
   * groups are passed over, and a tab inside one counts from its group's cell. While the tab that
   * shows is not in the row: the row's first tab, or its last.
   */
  cycle(dir: 1 | -1): void {
    const stops = this.stops;
    const n = stops.length;
    const here = this.openGroupId == null ? (this.activeGroup?.id ?? this.activeId) : this.activeId;
    const at = stops.findIndex((s) => s.id === here);
    const from = at >= 0 ? at : dir > 0 ? -1 : 0;
    for (let k = 1; k <= n; k++) {
      const stop = stops[(((from + dir * k) % n) + n) % n];
      if (isGroup(stop)) continue;
      if (stop.id !== this.activeId) this.activate(stop.id);
      return;
    }
  }

  /**
   * The active tab that was at `place` is gone: the tab that took its place in its row shows, else
   * the one before. A group left without tabs shows none while the bar shows it — its open screen
   * —, else a tab near the group shows.
   */
  #gone(place: Place): void {
    const group = this.#group(place.group);
    if (!group) {
      this.#land(place.index);
      return;
    }
    const next = group.tabs[Math.min(place.index, group.tabs.length - 1)];
    if (next) this.#show(next);
    else if (this.openGroupId === group.id) this.activeId = null;
    else this.#land(this.items.indexOf(group));
  }

  /**
   * What showed is gone, from position `index` of the top level: the nearest tab of the top level
   * shows (the next one first); when only groups are left, the tab that the nearest of them showed
   * last; in an empty bar, a new tab. With nothing but groups without tabs, no tab shows: the open
   * screen. The bar keeps its row.
   */
  #land(index: number): void {
    const items = this.items;
    this.activeId = null;
    if (items.length === 0) {
      this.openGroupId = null;
      this.newTab();
      return;
    }
    const near = [...items.slice(index), ...items.slice(0, index).reverse()];
    const shown =
      near.find((it): it is Tab => !isGroup(it)) ??
      near.flatMap((it) => (isGroup(it) ? [this.#lastOf(it)] : [])).find((t) => t !== undefined);
    if (shown) this.#show(shown);
  }

  // ───────────── closing ─────────────

  /** ⌘W: the active tab — in a group without tabs, the group itself. */
  close(): void {
    const group = this.openGroup;
    if (this.activeId != null) this.closeTab(this.activeId);
    // Never one that holds a tab (one dragged into it, not dropped yet): that asks first.
    else if (group && group.tabs.length === 0) this.#dropGroup(group.id);
  }

  /** Close a tab. When it was the last one of its group, ask whether the group goes with it. */
  closeTab(id: number): void {
    const place = this.placeOf(id);
    if (!place || !this.#tab(id)) return;
    const group = this.#group(place.group);
    if (group) this.#setTabs(group.id, group.tabs.filter((t) => t.id !== id));
    else this.items = this.items.filter((it) => it.id !== id);
    this.#release();
    if (this.activeId === id) this.#gone(place);
    this.#persist();
    if (group && group.tabs.length === 1) void this.#offerDelete(group.id);
  }

  /** Close the other tabs of the row of the tab `id`: its group's, or the top level's (groups stay). */
  closeOthers(id: number): void {
    if (!this.#tab(id)) return;
    const group = this.groupOf(id);
    if (this.#row(group).length < 2) return;
    this.activate(id);
    if (group) this.#setTabs(group.id, group.tabs.filter((t) => t.id === id));
    else this.items = this.items.filter((it) => isGroup(it) || it.id === id);
    this.#release();
    this.#persist();
  }

  /** The last tab of the group `id` closed: ask whether the group goes too. */
  async #offerDelete(id: number): Promise<void> {
    const group = this.#group(id);
    if (!group) return;
    let ok = false;
    try {
      ok = await ask(`Delete the group “${group.name}” too?`, { okLabel: 'Delete', cancelLabel: 'Keep' });
    } catch {
      return; // not running inside Tauri: the group stays
    }
    // Asked a moment ago: only a group that is still there, and still empty.
    if (ok && this.#group(id)?.tabs.length === 0) this.#dropGroup(id);
  }

  // ───────────── groups ─────────────

  /**
   * A new group, its name ready to be typed: empty and last in the bar, or — with a tab — holding
   * that tab, where the tab was (after the group it came out of). The bar shows its row.
   */
  newGroup(withTab?: number): void {
    const id = this.#nextId++;
    const from = withTab == null ? null : this.placeOf(withTab);
    const tab = withTab == null ? null : this.#tab(withTab);
    if (from && tab) {
      let items = this.items;
      let at = from.index;
      if (from.group == null) {
        items = items.filter((it) => it.id !== tab.id);
      } else {
        at = items.findIndex((it) => it.id === from.group) + 1;
        items = items.map((it) =>
          isGroup(it) && it.id === from.group ? { ...it, tabs: it.tabs.filter((t) => t.id !== tab.id) } : it,
        );
      }
      this.items = [...items.slice(0, at), { id, name: NEW_GROUP_NAME, tabs: [tab] }, ...items.slice(at)];
    } else {
      this.items = [...this.items, { id, name: NEW_GROUP_NAME, tabs: [] }];
    }
    // Its name is typed before its tabs, in its own row.
    this.enter(id);
    this.renaming = id;
    this.#persist();
  }

  /** Name a group and end the typing. A blank name keeps the one it has. */
  renameGroup(id: number, name: string): void {
    if (this.renaming === id) this.renaming = null;
    const group = this.#group(id);
    const text = name.trim();
    if (!group || !text || text === group.name) return;
    this.items = this.items.map((it) => (isGroup(it) && it.id === id ? { ...it, name: text } : it));
    this.#persist();
  }

  /** Delete a group and close its tabs. One that has tabs asks first. */
  async deleteGroup(id: number): Promise<void> {
    const group = this.#group(id);
    if (!group) return;
    const n = group.tabs.length;
    if (n > 0) {
      let ok = false;
      try {
        ok = await ask(`Delete the group “${group.name}” and close its ${n} tab${n === 1 ? '' : 's'}?`, {
          kind: 'warning',
          okLabel: 'Delete',
          cancelLabel: 'Cancel',
        });
      } catch {
        return;
      }
      if (!ok) return;
    }
    this.#dropGroup(id);
  }

  #dropGroup(id: number): void {
    const index = this.items.findIndex((it) => isGroup(it) && it.id === id);
    if (index < 0) return;
    /** The tab that shows is one of its tabs. */
    const held = this.activeGroup?.id === id;
    this.items = this.items.filter((it) => it.id !== id);
    if (this.openGroupId === id) this.openGroupId = null;
    this.#lastIn.delete(id);
    if (this.renaming === id) this.renaming = null;
    this.#release();
    // What showed went with it, or nothing showed: a tab near it.
    if (held || this.activeId == null) this.#land(index);
    this.#persist();
  }

  /**
   * Dissolve a group: its tabs take its place at the top level. One whose repository has a tab there
   * already joins that tab.
   */
  ungroup(id: number): void {
    const index = this.items.findIndex((it) => isGroup(it) && it.id === id);
    const group = this.items[index];
    if (!group || !isGroup(group)) return;
    const top = new Set(this.#row(null).map((t) => t.repo));
    const moved = group.tabs.filter((t) => !t.repo || !top.has(t.repo));
    const active = this.activeGroup?.id === id ? this.active : null;
    this.items = [...this.items.slice(0, index), ...moved, ...this.items.slice(index + 1)];
    if (this.openGroupId === id) this.openGroupId = null;
    this.#lastIn.delete(id);
    if (this.renaming === id) this.renaming = null;
    if (active && !moved.includes(active)) {
      // The tab that showed joined the top-level tab of its repository: that one shows.
      const twin = this.#row(null).find((t) => t.repo === active.repo);
      if (twin) this.#show(twin);
      else this.#land(index);
    } else if (this.activeId == null) {
      this.#land(index);
    }
    this.#persist();
  }

  // ───────────── moving ─────────────

  /**
   * Put the tab or group `id` at `to`: `to.index` counts the other items of that row. Every step of a
   * drag in the bar comes through here, to be followed by `drop` once the tab rests. A group stays at
   * the top level. The bar keeps its row and the page stays, also when the active tab leaves the row.
   */
  move(id: number, to: Place): void {
    const from = this.placeOf(id);
    if (!from || (from.group === to.group && from.index === to.index)) return;
    const item = from.group == null ? this.items[from.index] : this.#tab(id);
    if (!item || (to.group != null && (isGroup(item) || !this.#group(to.group)))) return;
    const within = (i: number, n: number) => Math.max(0, Math.min(i, n));
    let items =
      from.group == null
        ? this.items.filter((it) => it.id !== id)
        : this.items.map((it) =>
            isGroup(it) && it.id === from.group ? { ...it, tabs: it.tabs.filter((t) => t.id !== id) } : it,
          );
    if (to.group == null) {
      const at = within(to.index, items.length);
      items = [...items.slice(0, at), item, ...items.slice(at)];
    } else {
      items = items.map((it) => {
        if (!isGroup(it) || it.id !== to.group) return it;
        const at = within(to.index, it.tabs.length);
        return { ...it, tabs: [...it.tabs.slice(0, at), item as Tab, ...it.tabs.slice(at)] };
      });
    }
    this.items = items;
    if (this.activeId === id && to.group != null) this.#lastIn.set(to.group, id);
    this.#persist();
  }

  /**
   * The tab `id` came to rest where a move put it: another tab of its repository in the same row
   * gives way to it.
   */
  drop(id: number): void {
    const tab = this.#tab(id);
    if (!tab) return;
    const group = this.groupOf(id);
    if (!tab.repo) return;
    const twin = this.#row(group).find((t) => t.id !== id && t.repo === tab.repo);
    if (!twin) return;
    if (group) this.#setTabs(group.id, group.tabs.filter((t) => t !== twin));
    else this.items = this.items.filter((it) => it !== twin);
    // In the twin's place.
    if (this.activeId === twin.id) this.#show(tab);
    else if (group && this.#lastIn.get(group.id) === twin.id) this.#lastIn.set(group.id, id);
    this.#persist();
  }

  /** Move a tab to the end of a group, or out of its group (null): to the top level, right after it. */
  moveTo(id: number, group: number | null): void {
    const from = this.placeOf(id);
    if (!from || from.group === group || !this.#tab(id)) return;
    const target = this.#group(group);
    if (group != null && !target) return;
    const index = target ? target.tabs.length : this.items.findIndex((it) => it.id === from.group) + 1;
    this.move(id, { group, index });
    this.drop(id);
  }

  // ───────────── recent, recently closed, session ─────────────

  forgetRecent(path: string): void {
    this.recent = this.recent.filter((r) => r.path !== path);
    saveJSON('recent', this.recent);
  }

  forgetClosed(path: string): void {
    this.#setClosed(this.closed.filter((r) => r.path !== path));
  }

  /** Keep `list` as the recently closed: the first entry of each path, the newest few. */
  #setClosed(list: ClosedRepo[]): void {
    const seen = new Set<string>();
    const keep: ClosedRepo[] = [];
    for (const r of list) {
      if (seen.has(r.path)) continue;
      seen.add(r.path);
      keep.push(r);
    }
    this.closed = keep.slice(0, MAX_CLOSED);
    saveJSON('closed', this.closed);
  }

  /**
   * Startup: the previous session's bar, else a new tab. Folders given at launch (CLI, Dock) take the
   * place of that session's top-level tabs; its groups come back either way. The bar shows the top
   * level; the first tab shows as soon as it is there — one inside a group without its group being
   * opened — and the rest opens behind it. A path that no longer opens is left out of
   * the stored bar — unless nothing opens at all (the volume is not mounted) and the bar is left
   * alone meanwhile: then the stored session stays as it is.
   */
  async restore(): Promise<void> {
    let startup: string[] = [];
    try {
      startup = await api.startupPaths();
    } catch {
      /* ignore */
    }
    const stored = loadJSON<unknown>('tabs', []);
    const saved = (Array.isArray(stored) ? stored : []).filter(
      (s): s is Saved => typeof s === 'string' || isSavedGroup(s),
    );
    const first = () => {
      const tab = this.tabs[0];
      if (tab && this.activeId == null && this.openGroupId == null) this.#show(tab);
    };
    this.#pending = startup.length > 0 ? [...startup, ...saved.filter(isSavedGroup)] : saved;
    while (this.#pending.length > 0) {
      const s = this.#pending[0];
      if (typeof s === 'string') {
        await this.open(s, { newTab: true, group: null, background: true });
        this.#pending = this.#pending.slice(1);
      } else {
        // From here on the group in the bar stands for its entry.
        const id = this.#nextId++;
        this.items = [...this.items, { id, name: s.group.trim() || NEW_GROUP_NAME, tabs: [] }];
        this.#pending = this.#pending.slice(1);
        const fill = { id, paths: s.tabs.filter((p): p is string => typeof p === 'string') };
        this.#filling = fill;
        // A group deleted meanwhile takes what was left of it along.
        while (fill.paths.length > 0 && this.#group(id)) {
          await this.open(fill.paths[0], { newTab: true, group: id, background: true });
          fill.paths = fill.paths.slice(1);
          first();
        }
        this.#filling = null;
      }
      first();
    }
    if (this.items.length === 0) this.newTab();
    this.restored = true;
    // Stored meanwhile, with what was still to come: once more, without what failed to open.
    if (this.#stored) this.#persist();
  }

  #remember(path: string, name: string): void {
    const rest = this.recent.filter((r) => r.path !== path);
    this.recent = [{ path, name, openedAt: Date.now() }, ...rest].slice(0, MAX_RECENT);
    saveJSON('recent', this.recent);
    // Open again: no longer closed.
    if (this.closed.some((r) => r.path === path)) this.forgetClosed(path);
  }

  /** Store the bar: its repositories (new tabs are not kept), and what `restore` has yet to open. */
  #persist(): void {
    const paths = (tabs: Tab[]) => tabs.flatMap((t) => (t.repo ? [t.repo.id] : []));
    const fill = this.#filling;
    const bar = this.items.flatMap((it): Saved[] => {
      if (!isGroup(it)) return paths([it]);
      const own = paths(it.tabs);
      // A path being opened is in both until its open returns.
      const rest = fill?.id === it.id ? fill.paths.filter((p) => !own.includes(p)) : [];
      return [{ group: it.name, tabs: [...own, ...rest] }];
    });
    const pending = this.#pending.filter((s) => typeof s !== 'string' || !bar.includes(s));
    saveJSON('tabs', [...bar, ...pending]);
    this.#stored = true;
  }
}

export const app = new AppState();
