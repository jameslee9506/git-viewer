import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  worker: { format: 'es' },
  // The dependency scan doesn't follow `new Worker(new URL(...))`; pre-bundle shiki so the first
  // highlight in dev doesn't trigger a dependency re-optimization + page reload.
  optimizeDeps: { include: ['shiki/core', 'shiki/engine/oniguruma', 'shiki/langs', 'shiki/wasm'] },
  build: {
    // The webview is the system WebKit (WKWebView). Syntax Safari 16.4 supports ships as-is and
    // APIs/CSS (structuredClone, inert, color-mix()) are never polyfilled, so
    // `bundle.macOS.minimumSystemVersion` in src-tauri/tauri.conf.json is 13.3, the macOS release
    // that ships Safari 16.4. Change the two together (and README's requirement).
    target: 'safari16.4',
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 4000,
  },
});
