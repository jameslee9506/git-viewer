// Global user settings, persisted. Mutate fields directly: `settings.diffMode = 'split'`.
import type { GraphOrder } from '../types';
import { debouncedSaver, loadJSON } from './persist';

export type ThemePreference = 'system' | 'light' | 'dark';

export function normalizeTheme(value: unknown): ThemePreference {
  return value === 'light' || value === 'dark' ? value : 'system';
}

export interface Settings {
  theme: ThemePreference;
  dateFormat: 'relative' | 'absolute';
  graphOrder: GraphOrder;
  /** 0 = unlimited */
  maxCommits: number;
  showRemotes: boolean;
  showTags: boolean;
  showStashes: boolean;
  pinHeadLane: boolean;
  diffMode: 'unified' | 'split';
  ignoreWhitespace: boolean;
  contextLines: number;
  fileListMode: 'list' | 'tree';
  detailsVisible: boolean;
  sidebarWidth: number;
  detailsWidth: number;
  /** Graph column widths in px. */
  colGraph: number;
  colAuthor: number;
  colDate: number;
  colSha: number;
  /** Branch / tag column width in px (see refsColumn). */
  colRefs: number;
  /**
   * Ref labels in a branch / tag column left of the graph, each linked to its commit by a dotted
   * line; off: inline before the subject.
   */
  refsColumn: boolean;
  showAuthor: boolean;
  showDate: boolean;
  showSha: boolean;
  /** Webview zoom factor (0.7 – 1.6), applied with getCurrentWebview().setZoom(). */
  zoom: number;
  /** Render merge commits' subjects in a muted color. */
  showMergesMuted: boolean;
}

export const DEFAULT_SETTINGS: Settings = {
  theme: 'system',
  dateFormat: 'relative',
  graphOrder: 'date',
  maxCommits: 50000,
  showRemotes: true,
  showTags: true,
  showStashes: true,
  pinHeadLane: true,
  diffMode: 'unified',
  ignoreWhitespace: false,
  contextLines: 3,
  fileListMode: 'list',
  detailsVisible: true,
  sidebarWidth: 240,
  detailsWidth: 380,
  colGraph: 120,
  colAuthor: 140,
  colDate: 72,
  colSha: 64,
  colRefs: 140,
  refsColumn: true,
  showAuthor: true,
  showDate: true,
  showSha: true,
  zoom: 1,
  showMergesMuted: true,
};

const restored = loadJSON('settings', DEFAULT_SETTINGS);
export const settings: Settings = $state({
  ...DEFAULT_SETTINGS,
  ...restored,
  theme: normalizeTheme(restored?.theme),
});

const save = debouncedSaver('settings');

$effect.root(() => {
  $effect(() => {
    // Read every field so the effect re-runs on any change.
    save($state.snapshot(settings));
  });
});

export function resetSettings(): void {
  Object.assign(settings, DEFAULT_SETTINGS);
}
