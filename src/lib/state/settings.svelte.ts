// Global user settings, persisted. Mutate fields directly: `settings.diffMode = 'split'`.
import type { GraphOrder } from '../types';
import { debouncedSaver, loadJSON } from './persist';

export interface Settings {
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
  sidebarVisible: boolean;
  detailsVisible: boolean;
  sidebarWidth: number;
  detailsWidth: number;
  /** Graph column widths in px. */
  colGraph: number;
  colAuthor: number;
  colDate: number;
  colSha: number;
  showAuthor: boolean;
  showDate: boolean;
  showSha: boolean;
  /** Webview zoom factor (0.7 – 1.6), applied with getCurrentWebview().setZoom(). */
  zoom: number;
  /** Render merge commits' subjects in a muted color. */
  showMergesMuted: boolean;
}

export const DEFAULT_SETTINGS: Settings = {
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
  sidebarVisible: true,
  detailsVisible: true,
  sidebarWidth: 240,
  detailsWidth: 380,
  colGraph: 120,
  colAuthor: 140,
  colDate: 72,
  colSha: 64,
  showAuthor: true,
  showDate: true,
  showSha: true,
  zoom: 1,
  showMergesMuted: true,
};

export const settings: Settings = $state(loadJSON('settings', DEFAULT_SETTINGS));

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
