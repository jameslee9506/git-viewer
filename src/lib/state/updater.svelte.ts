// App updates are independent of repositories. Only an explicit click downloads/installs anything.
import { isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { errorMessage } from '../api';
import { saveJSON } from './persist';
import { settings } from './settings.svelte';
import { toast } from './ui.svelte';

type Phase = 'idle' | 'checking' | 'available' | 'downloading' | 'installing' | 'restart' | 'restarting';

const CHECK_TIMEOUT_MS = 10_000;
const DOWNLOAD_TIMEOUT_MS = 5 * 60_000;

class UpdaterState {
  phase = $state<Phase>('idle');
  version = $state('');
  /** null when the server does not send Content-Length. */
  progress = $state<number | null>(null);
  error = $state<string | null>(null);

  visible = $derived(this.phase !== 'idle' && this.phase !== 'checking');
  busy = $derived(this.phase === 'downloading' || this.phase === 'installing' || this.phase === 'restarting');

  // A Tauri Resource owns a backend handle; keep it out of reactive proxies.
  #update: Update | null = null;
  #started = false;
  #checkSeq = 0;
  #checkAgain = false;

  /** One non-blocking startup check; retry when an offline machine comes back online. */
  start(): () => void {
    if (import.meta.env.DEV || !isTauri() || this.#started) return () => {};
    this.#started = true;
    const online = () => {
      if (this.phase === 'checking') this.#checkAgain = true;
      else void this.#check();
    };
    window.addEventListener('online', online);
    void this.#check();
    return () => {
      window.removeEventListener('online', online);
      this.#started = false;
      this.#checkSeq++;
      this.#checkAgain = false;
      // An in-flight install owns the resource until it settles.
      if (!this.busy) {
        void this.#release();
        if (this.phase !== 'restart') this.phase = 'idle';
      }
    };
  }

  async #check(): Promise<void> {
    if (!this.#started || this.phase !== 'idle') return;
    const seq = ++this.#checkSeq;
    this.phase = 'checking';
    try {
      const update = await check({ timeout: CHECK_TIMEOUT_MS });
      if (!this.#started || seq !== this.#checkSeq) {
        await update?.close().catch(() => {});
        return;
      }
      this.#update = update;
      this.version = update?.version ?? '';
      this.error = null;
      this.phase = update ? 'available' : 'idle';
    } catch {
      // Offline, no published release yet, or a local build without a configured endpoint.
      // An optional background check must not interrupt opening repositories.
      if (seq === this.#checkSeq) this.phase = 'idle';
    } finally {
      if (this.#started && seq === this.#checkSeq) {
        // A connection can recover before the original offline request has timed out.
        const again = this.#checkAgain;
        this.#checkAgain = false;
        if (again && this.phase === 'idle') void this.#check();
      }
    }
  }

  async install(): Promise<void> {
    if (!this.#started || this.busy) return;
    if (this.phase === 'restart') {
      await this.#restart();
      return;
    }
    if (this.phase !== 'available' || !this.#update) return;

    const update = this.#update;
    this.phase = 'downloading';
    this.progress = null;
    this.error = null;
    let downloaded = 0;
    let total: number | undefined;
    let receiveProgress = true;
    try {
      await update.downloadAndInstall((event) => {
        if (!receiveProgress) return;
        switch (event.event) {
          case 'Started':
            downloaded = 0;
            total = event.data.contentLength;
            this.progress = total && total > 0 ? 0 : null;
            break;
          case 'Progress':
            downloaded += event.data.chunkLength;
            // Only publish whole percentages, not every downloaded chunk.
            this.progress = total && total > 0 ? Math.min(100, Math.floor(downloaded / total * 100)) : null;
            break;
          case 'Finished':
            // This event ends the download, not signature verification or installation.
            this.phase = 'installing';
            this.progress = null;
            break;
        }
      }, { timeout: DOWNLOAD_TIMEOUT_MS }).finally(() => {
        // A queued channel message must not change the state after this attempt has settled.
        receiveProgress = false;
      });
      await this.#release();
      this.phase = 'restart';
      if (this.#started) await this.#restart();
    } catch (e) {
      this.phase = 'available';
      this.progress = null;
      this.error = errorMessage(e);
      if (this.#started) toast(`Update failed: ${this.error}`);
    } finally {
      if (!this.#started) {
        await this.#release();
        if (this.phase !== 'restart') this.phase = 'idle';
      }
    }
  }

  async #restart(): Promise<void> {
    this.phase = 'restarting';
    this.error = null;
    try {
      // Settings normally save after 300 ms; don't lose an edit made during the download.
      saveJSON('settings', $state.snapshot(settings));
      await relaunch();
    } catch (e) {
      // The new app is already installed. The next click must only retry restarting it.
      this.phase = 'restart';
      this.error = errorMessage(e);
      if (this.#started) toast(`Restart failed: ${this.error}`);
    }
  }

  async #release(): Promise<void> {
    const update = this.#update;
    this.#update = null;
    await update?.close().catch(() => {});
  }
}

export const updater = new UpdaterState();
