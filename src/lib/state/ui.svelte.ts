// Global transient UI state: context menu + toast. Rendered once by ContextMenu.svelte / Toast.svelte in App.
import type { Component } from 'svelte';

export type MenuEntry =
  | {
      label: string;
      /** Muted words after the label, where the name alone does not tell what the entry does. */
      note?: string;
      /** Lucide icon component (import X from '@lucide/svelte/icons/x'). */
      icon?: Component<any>;
      /** Display-only shortcut hint, e.g. '⌘C'. */
      hint?: string;
      disabled?: boolean;
      /** Runs on click / Enter. An entry with `children` opens them instead. */
      action?: () => void;
      /** A submenu, opened beside the entry (one level deep: its entries have none of their own). */
      children?: MenuEntry[];
    }
  | { separator: true };

export const contextMenu = $state({
  open: false,
  x: 0,
  y: 0,
  items: [] as MenuEntry[],
  /** Counts the menus shown: `open && seq === n` tells the opener of menu `n` that it is still up. */
  seq: 0,
  /**
   * The button a dropdown hangs from, if the menu is one: a press on it does not close the menu, so
   * that the button itself can (its click toggles: `open && seq === n`).
   */
  anchor: null as Element | null,
});

/**
 * Show `items` at the point `at` of the window (the menu flips and clamps to stay inside it), as a
 * dropdown of the button `anchor` when one is given. Returns the menu's `contextMenu.seq`.
 */
export function showMenu(at: { x: number; y: number }, items: MenuEntry[], anchor: Element | null = null): number {
  contextMenu.items = items;
  contextMenu.x = at.x;
  contextMenu.y = at.y;
  contextMenu.anchor = anchor;
  contextMenu.open = true;
  return ++contextMenu.seq;
}

/** Show `items` at the pointer of `e`. Returns the menu's `contextMenu.seq`. */
export function showContextMenu(e: MouseEvent, items: MenuEntry[]): number {
  e.preventDefault();
  e.stopPropagation();
  return showMenu({ x: e.clientX, y: e.clientY }, items);
}

export function hideContextMenu(): void {
  contextMenu.open = false;
  contextMenu.anchor = null;
}

export const toastState = $state({ message: '', seq: 0 });

/** Brief, terse feedback (e.g. 'Copied'). */
export function toast(message: string): void {
  toastState.message = message;
  toastState.seq++;
}
