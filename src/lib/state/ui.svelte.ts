// Global transient UI state: context menu + toast. Rendered once by ContextMenu.svelte / Toast.svelte in App.
import type { Component } from 'svelte';

export type MenuEntry =
  | {
      label: string;
      /** Lucide icon component (import X from '@lucide/svelte/icons/x'). */
      icon?: Component<any>;
      /** Display-only shortcut hint, e.g. '⌘C'. */
      hint?: string;
      disabled?: boolean;
      action: () => void;
    }
  | { separator: true };

export const contextMenu = $state({
  open: false,
  x: 0,
  y: 0,
  items: [] as MenuEntry[],
});

export function showContextMenu(e: MouseEvent, items: MenuEntry[]): void {
  e.preventDefault();
  e.stopPropagation();
  contextMenu.items = items;
  contextMenu.x = e.clientX;
  contextMenu.y = e.clientY;
  contextMenu.open = true;
}

export function hideContextMenu(): void {
  contextMenu.open = false;
}

export const toastState = $state({ message: '', seq: 0 });

/** Brief, terse feedback (e.g. 'Copied'). */
export function toast(message: string): void {
  toastState.message = message;
  toastState.seq++;
}
