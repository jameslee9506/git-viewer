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
  /** The tab that shows. Null while the open group has no tab: its empty state. */
  activeId = $state<number | null>(null);
  /**
   * The group the bar shows unfolded: the one that holds the active tab, or one without tabs that was
   * picked. Null while a top-level tab shows.
   */
  openGroupId = $state<number | null>(null);
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
  /**
   * What ⌘1…⌘9, ⌃Tab and ⌥⌘← / ⌥⌘→ step through, in bar order: every tab, a group's tabs taking
   * the group's place. A group without tabs is one stop itself.
   */
  stops = $derived(this.items.flatMap((it): Item[] => (isGroup(it) && it.tabs.length > 0 ? it.tabs : [it])));
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
   * Open a repo (any path inside it) where the user is: in the open group, else at the top level.
   * A tab of that repository already in that row is focused instead. One in another row stays, and
   * the new tab shows the same repository through the same store.
   */
  async open(path: string, opts: OpenOptions = {}): Promise<void> {
    // A clone's tab shows a spinner of its own.
    if (opts.into == null) this.opening = path;
    this.error = null;
    try {
      const info = await api.openRepo(path);
      // Looked up now, not before the wait: the user may have moved on.
      const into = opts.into == null ? null : this.#tab(opts.into);
      const group =
        opts.into != null
          ? into && this.groupOf(into.id)
          : opts.group === undefined
            ? this.openGroup
            : this.#group(opts.group);
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
        if (!opts.background) this.activate(existing.id);
        return;
      }
      const repo = held ?? this.#adopt(new RepoStore(info));
      const cur = this.active;
      // A clone's repository fills the clone's tab. Otherwise the active new tab of that row is reused
      // (same id) unless a tab of its own was asked for, or a clone runs for it: that one fills it.
      const reuse =
        into ??
        (cur && cur.repo === null && !opts.newTab && row.includes(cur) && !this.cloneFor(cur.id) ? cur : null);
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
      // Also in the background when it is the first tab of the open group: that group shows no tab.
      const first = group != null && group.id === this.openGroupId && this.activeId == null;
      if (!opts.background || first) this.activate(tab.id);
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

  /** Show a tab; one inside a group opens that group. */
  activate(id: number): void {
    if (!this.#tab(id)) return;
    const group = this.groupOf(id);
    this.activeId = id;
    this.openGroupId = group?.id ?? null;
    if (group) this.#lastIn.set(group.id, id);
  }

  /** Open a group: the tab it showed last (else its first). Without tabs, its empty state. */
  enter(id: number): void {
    const group = this.#group(id);
    if (!group) return;
    const last = this.#lastIn.get(id);
    const tab = group.tabs.find((t) => t.id === last) ?? group.tabs[0];
    this.openGroupId = id;
    this.activeId = tab?.id ?? null;
    if (tab) this.#lastIn.set(id, tab.id);
  }

  #go(stop: Item | undefined): void {
    if (!stop) return;
    if (isGroup(stop)) this.enter(stop.id);
    else this.activate(stop.id);
  }

  /** Go to a stop by 0-based position (Cmd+1..9; 9 = the last). */
  activateIndex(i: number): void {
    const stops = this.stops;
    this.#go(i >= 8 ? stops[stops.length - 1] : stops[i]);
  }

  cycle(dir: 1 | -1): void {
    const stops = this.stops;
    const n = stops.length;
    if (n < 2) return;
    const cur = this.activeId ?? this.openGroupId;
    const idx = stops.findIndex((s) => s.id === cur);
    this.#go(stops[(idx + dir + n) % n]);
  }

  /**
   * The active tab that was at `place` is gone: the tab that took its place in its row shows, else
   * the one before. A group left without tabs stays open, empty.
   */
  #leave(place: Place): void {
    const group = this.#group(place.group);
    if (!group) {
      this.#land(place.index);
      return;
    }
    const next = group.tabs[Math.min(place.index, group.tabs.length - 1)];
    this.activeId = next?.id ?? null;
    if (next) this.#lastIn.set(group.id, next.id);
  }

  /**
   * What showed at the top level, at position `index`, is gone: go to the nearest tab there (the next
   * one first), into the nearest group when only groups are left, and to a new tab in an empty bar.
   */
  #land(index: number): void {
    const items = this.items;
    this.activeId = null;
    this.openGroupId = null;
    if (items.length === 0) {
      this.newTab();
      return;
    }
    const isTab = (it: Item) => !isGroup(it);
    const tab = items.slice(index).find(isTab) ?? items.slice(0, index).reverse().find(isTab);
    this.#go(tab ?? items[Math.min(index, items.length - 1)]);
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
    if (this.activeId === id) this.#leave(place);
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
   * that tab, where the tab was (after the group it came out of).
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
      this.activate(tab.id);
    } else {
      this.items = [...this.items, { id, name: NEW_GROUP_NAME, tabs: [] }];
      this.enter(id);
    }
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
    this.items = this.items.filter((it) => it.id !== id);
    this.#lastIn.delete(id);
    if (this.renaming === id) this.renaming = null;
    this.#release();
    if (this.openGroupId === id) this.#land(index);
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
    const wasOpen = this.openGroupId === id;
    const active = wasOpen ? this.active : null;
    this.items = [...this.items.slice(0, index), ...moved, ...this.items.slice(index + 1)];
    this.#lastIn.delete(id);
    if (this.renaming === id) this.renaming = null;
    if (wasOpen) {
      // The tab that showed, or the top-level tab of its repository that it joined.
      const shown = active && (moved.includes(active) ? active : this.#row(null).find((t) => t.repo === active.repo));
      if (shown) this.activate(shown.id);
      else this.#land(index);
    }
    this.#persist();
  }

  // ───────────── moving ─────────────

  /**
   * Put the tab or group `id` at `to`: `to.index` counts the other items of that row. Every step of a
   * drag in the bar comes through here, to be followed by `drop` once the tab rests. A group stays at
   * the top level. The open group follows the active tab.
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
    if (this.activeId === id) {
      this.openGroupId = to.group;
      if (to.group != null) this.#lastIn.set(to.group, id);
    }
    this.#persist();
  }

  /**
   * The tab `id` came to rest where a move put it. Put into the open group while that showed no tab,
   * it shows — not before: a tab dragged across an empty open group only passes through it. Another
   * tab of its repository in the same row gives way to it.
   */
  drop(id: number): void {
    const tab = this.#tab(id);
    if (!tab) return;
    const group = this.groupOf(id);
    if (group && group.id === this.openGroupId && this.activeId == null) this.activate(id);
    if (!tab.repo) return;
    const twin = this.#row(group).find((t) => t.id !== id && t.repo === tab.repo);
    if (!twin) return;
    if (group) this.#setTabs(group.id, group.tabs.filter((t) => t !== twin));
    else this.items = this.items.filter((it) => it !== twin);
    if (this.activeId === twin.id) this.activate(id);
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
   * place of that session's top-level tabs; its groups come back either way. The first stop shows as
   * soon as it is there, and the rest opens behind it. A path that no longer opens is left out of
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
      if (this.activeId == null && this.openGroupId == null) this.#go(this.stops[0]);
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
          // Not its open screen while tabs are still to come.
          if (this.#group(id)?.tabs.length) first();
        }
        this.#filling = null;
      }
      first();
    }
    if (this.items.length === 0) this.newTab();
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
