// App-level state: tabs (one RepoStore per tab, or a welcome tab), recent repos, session restore.
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { api, errorMessage } from '../api';
import { dirname } from '../util/format';
import { forgetRepo } from './cache';
import { loadJSON, saveJSON } from './persist';
import { RepoStore } from './repo.svelte';
import { toast } from './ui.svelte';

export interface Tab {
  id: number;
  /** null = welcome tab */
  repo: RepoStore | null;
}

export interface RecentRepo {
  path: string;
  name: string;
  /** unix ms */
  openedAt: number;
}

const MAX_RECENT = 20;

class AppState {
  tabs = $state.raw<Tab[]>([]);
  activeId = $state<number | null>(null);
  recent = $state.raw<RecentRepo[]>(loadJSON<RecentRepo[]>('recent', []));
  /** Path currently being opened (welcome screen spinner). */
  opening = $state<string | null>(null);
  error = $state<string | null>(null);

  #nextId = 1;

  active = $derived(this.tabs.find((t) => t.id === this.activeId) ?? null);

  /** Open a repo (any path inside it). Focuses an existing tab for the same repo. */
  async open(path: string, opts: { newTab?: boolean } = {}): Promise<void> {
    this.opening = path;
    this.error = null;
    try {
      const info = await api.openRepo(path);
      const existing = this.tabs.find((t) => t.repo?.id === info.path);
      if (existing) {
        // openRepo took a backend reference; the existing tab already holds one.
        void api.closeRepo(info.path).catch(() => {});
        this.activeId = existing.id;
        return;
      }
      const repo = new RepoStore(info);
      const cur = this.active;
      // Reuse the active welcome tab (same id: its page stays mounted) unless a new tab was asked for.
      const reuse = cur && cur.repo === null && !opts.newTab ? cur : null;
      const tab: Tab = { id: reuse ? reuse.id : this.#nextId++, repo };
      const others = reuse ? this.tabs.filter((t) => t.id !== reuse.id) : this.tabs;
      // Worktrees of one repository share commonDir (canonical, also for bare repos): keep their
      // tabs adjacent by inserting after the group's last tab (features-ux M14). Without a group a
      // reused welcome tab keeps its slot and a new tab goes last. Reverse loop: lib is ES2022.
      let at = reuse ? this.tabs.findIndex((t) => t.id === reuse.id) : others.length;
      for (let i = others.length - 1; i >= 0; i--) {
        if (others[i].repo?.info.commonDir === info.commonDir) {
          at = i + 1;
          break;
        }
      }
      this.tabs = [...others.slice(0, at), tab, ...others.slice(at)];
      this.activeId = tab.id;
      this.#remember(info.path, info.name);
      this.#persistTabs();
      void repo.init();
    } catch (e) {
      this.error = errorMessage(e);
      toast(this.error);
    } finally {
      if (this.opening === path) this.opening = null;
    }
  }

  /**
   * Finder folder panel; each picked folder opens like `open` (one tab per folder, like a drop).
   * Starts in `from`, else in the folder holding the active repo (or the last opened one).
   */
  async openWithDialog(opts: { newTab?: boolean; from?: string } = {}): Promise<void> {
    const near = this.active?.repo?.id ?? this.recent[0]?.path;
    const picked = await openDialog({
      directory: true,
      multiple: true,
      canCreateDirectories: false,
      defaultPath: opts.from ?? (near ? dirname(near) || '/' : undefined),
    });
    for (const path of picked ?? []) await this.open(path, { newTab: opts.newTab });
  }

  newTab(): void {
    const tab: Tab = { id: this.#nextId++, repo: null };
    this.tabs = [...this.tabs, tab];
    this.activeId = tab.id;
  }

  closeTab(id: number | null = this.activeId): void {
    if (id == null) return;
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx < 0) return;
    const closed = this.tabs[idx].repo;
    if (closed) {
      closed.dispose();
      forgetRepo(closed.id);
    }
    const rest = this.tabs.filter((t) => t.id !== id);
    if (rest.length === 0) {
      const tab: Tab = { id: this.#nextId++, repo: null };
      this.tabs = [tab];
      this.activeId = tab.id;
    } else {
      this.tabs = rest;
      if (this.activeId === id) this.activeId = rest[Math.min(idx, rest.length - 1)].id;
    }
    this.#persistTabs();
  }

  activate(id: number): void {
    if (this.tabs.some((t) => t.id === id)) this.activeId = id;
  }

  /** Activate tab by 0-based position (Cmd+1..9; 9 = last). */
  activateIndex(i: number): void {
    const t = i >= 8 ? this.tabs[this.tabs.length - 1] : this.tabs[i];
    if (t) this.activeId = t.id;
  }

  cycle(dir: 1 | -1): void {
    const n = this.tabs.length;
    if (n < 2) return;
    const idx = this.tabs.findIndex((t) => t.id === this.activeId);
    this.activeId = this.tabs[(idx + dir + n) % n].id;
  }

  /** Move a tab to a new position (drag reorder). */
  moveTab(id: number, to: number): void {
    const from = this.tabs.findIndex((t) => t.id === id);
    if (from < 0) return;
    const next = [...this.tabs];
    const [t] = next.splice(from, 1);
    next.splice(Math.max(0, Math.min(to, next.length)), 0, t);
    this.tabs = next;
    this.#persistTabs();
  }

  forgetRecent(path: string): void {
    this.recent = this.recent.filter((r) => r.path !== path);
    saveJSON('recent', this.recent);
  }

  /** Startup: CLI paths first, else the previous session's tabs, else a welcome tab. */
  async restore(): Promise<void> {
    let paths: string[] = [];
    try {
      paths = await api.startupPaths();
    } catch {
      /* ignore */
    }
    if (paths.length === 0) paths = loadJSON<string[]>('tabs', []);
    if (paths.length === 0) {
      this.newTab();
      return;
    }
    for (const p of paths) await this.open(p, { newTab: true });
    if (this.tabs.length === 0) this.newTab();
    else this.activeId = this.tabs[0].id;
  }

  #remember(path: string, name: string): void {
    const rest = this.recent.filter((r) => r.path !== path);
    this.recent = [{ path, name, openedAt: Date.now() }, ...rest].slice(0, MAX_RECENT);
    saveJSON('recent', this.recent);
  }

  #persistTabs(): void {
    saveJSON(
      'tabs',
      this.tabs.filter((t) => t.repo).map((t) => t.repo!.id),
    );
  }
}

export const app = new AppState();
