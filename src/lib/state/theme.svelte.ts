// CSS owns every color. Changing appearance never reloads graph data or highlight caches.
import { untrack } from 'svelte';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { saveJSON } from './persist';
import { settings, type ThemePreference } from './settings.svelte';

let refresh: (() => void) | undefined;

/** Re-read the chrome token after user.css is loaded or edited. */
export function refreshTheme(): void {
  refresh?.();
}

/** Install once before mounting the UI; return a disposer for hot reload. */
export function startTheme(): () => void {
  const media = window.matchMedia('(prefers-color-scheme: dark)');
  const root = document.documentElement;
  const native = isTauri() ? getCurrentWebviewWindow() : null;
  const swatch = document.createElement('canvas');
  swatch.width = swatch.height = 1;
  const paint = native ? swatch.getContext('2d', { willReadFrequently: true }) : null;
  let disposed = false;
  let pending = false;
  let syncing = false;
  let nativePreference: ThemePreference | undefined;

  function applyDocument(): void {
    root.dataset.theme = settings.theme === 'system'
      ? (media.matches ? 'dark' : 'light')
      : settings.theme;
  }

  function backgroundColor(): [number, number, number, number] | null {
    if (!paint) return null;
    // Tauri accepts only hex/bytes. Let WebKit resolve arbitrary user.css colors (including
    // aliases, rgb(), and color-mix()) instead of maintaining a second CSS color parser.
    paint.clearRect(0, 0, 1, 1);
    paint.fillStyle = getComputedStyle(root).backgroundColor;
    paint.fillRect(0, 0, 1, 1);
    const [r, g, b, a] = paint.getImageData(0, 0, 1, 1).data;
    return [r, g, b, a];
  }

  // Serialize native changes and coalesce rapid toggles. An older IPC completion must never
  // leave the title bar in the opposite appearance to the current document.
  async function syncNative(): Promise<void> {
    if (!native || syncing || disposed) return;
    syncing = true;
    try {
      while (pending && !disposed) {
        pending = false;
        const preference = settings.theme;
        if (nativePreference !== preference) {
          try {
            // On macOS this also changes the app's menus and dialogs. For System, release the
            // override: forcing the resolved color would pin matchMedia to that appearance.
            await native.setTheme(preference === 'system' ? null : preference);
            nativePreference = preference;
          } catch {
            // Keep the document usable if native appearance is unavailable; retry next change.
          }
        }
        if (disposed) return;
        if (preference !== settings.theme) {
          pending = true;
          continue;
        }
        // Re-read after releasing a forced native appearance; WebKit may also notify us later.
        applyDocument();
        const background = backgroundColor();
        if (background) await native.setBackgroundColor(background).catch(() => {});
      }
    } finally {
      syncing = false;
    }
  }

  function apply(): void {
    applyDocument();
    pending = true;
    void syncNative();
  }

  function onSystemChange(): void {
    if (settings.theme === 'system') apply();
  }

  media.addEventListener('change', onSystemChange);
  refresh = apply;
  apply();

  const stop = $effect.root(() => {
    $effect(() => {
      // Track only preference: resizing columns and changing graph options must not touch theme.
      void settings.theme;
      untrack(() => {
        // Save immediately, including reset/direct mutations, so quitting after a switch keeps it.
        saveJSON('settings', $state.snapshot(settings));
        apply();
      });
    });
  });

  return () => {
    disposed = true;
    stop();
    media.removeEventListener('change', onSystemChange);
    if (refresh === apply) refresh = undefined;
  };
}
