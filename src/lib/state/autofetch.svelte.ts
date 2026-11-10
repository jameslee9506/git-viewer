// Background fetch ("auto-fetch"). The remotes of the repository whose tab is active are fetched every
// AUTO_FETCH_MS while the window shows, and soon after a tab comes up whose last fetch is older. Quietly:
// no toast, nothing disabled; the footer shows that it runs, when it last went through and which remote
// fails. It is the one git write nobody clicks for, and the only one outside the per-repo operation
// queue (ops.svelte.ts): staging and committing never wait for the network. A fetch / pull / push of the
// user's stops it first (`yield`), and none starts while an operation runs.
import { api, errorMessage } from '../api';
import type { Remote, RemoteFetch } from '../types';
import type { RepoStore } from './repo.svelte';
import { settings } from './settings.svelte';

/** The remotes of the active tab's repository are fetched this often. */
export const AUTO_FETCH_MS = 5 * 60_000;
/** A remote that failed once is tried again this soon: the network was not up yet (a wake), a hiccup. */
const RETRY_MS = 60_000;
/** One that keeps failing waits the interval, then twice as long after every further failure, up to this. */
const MAX_WAIT_MS = 60 * 60_000;
/** Failures in a row before a remote counts as failing for the footer: a hiccup shows nothing. */
const WARN_FAILURES = 2;
/**
 * A fetch takes along the remotes that come due within this, so that all of them keep one beat
 * whenever one was last fetched by itself (the rest of a fetch that was cut short, a pull, a remote
 * that works again). More than half the interval: of two remotes out of step, one is always within
 * this of the other's turn, so they fall in step within two fetches instead of taking turns for good
 * — as long as a fetch takes less than the rest of the interval (remotes that each need minutes
 * every time may keep their own turns, each still once per interval).
 */
const TOGETHER_MS = (AUTO_FETCH_MS * 2) / 3;
/**
 * A fetch that is due starts once things have been still this long: a tab passed through on the way to
 * another one fetches nothing, and neither does the moment an operation ends.
 */
const SETTLE_MS = 1500;
/** The remotes that a fetch which was cut short did not get to wait this long. */
const RESUME_MS = 10_000;
/** The schedule is checked against the clock this often: timers stand still while the machine sleeps. */
const CHECK_MS = 60_000;
/** A cancel request is repeated this often until the background fetch has ended. */
const CANCEL_RETRY_MS = 250;

/** A remote git can fetch from: one with a url. */
type FetchRemote = Remote & { url: string };

/** The last fetch attempt of a remote. */
interface RemoteState {
  /** The url it was made with: a remote that points elsewhere now starts anew. */
  url: string;
  /** When it ended, unix ms (0: it is due again whatever it waits for). */
  at: number;
  /** Failed attempts in a row (0: it went through). */
  failures: number;
  /** git's message when it failed. */
  error: string | null;
}

export interface FetchFailure {
  remote: string;
  /** git's own message. */
  error: string;
}

/**
 * The remotes of `repo` to fetch. None in a bare repo: the app offers no fetch there, and a mirror's
 * refspec would move its branches themselves.
 */
function fetchable(repo: RepoStore): FetchRemote[] {
  if (repo.info.isBare) return [];
  return (repo.refs?.remoteList ?? []).filter((r): r is FetchRemote => r.url != null);
}

/** How long a remote waits after an attempt that left it with `failures` of them in a row. */
function waitAfter(failures: number): number {
  if (failures === 0) return AUTO_FETCH_MS;
  if (failures === 1) return RETRY_MS;
  return Math.min(AUTO_FETCH_MS * 2 ** (failures - 2), MAX_WAIT_MS);
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** The instances by common dir. */
const shared = new Map<string, AutoFetch>();

/**
 * How often the network has reported back (see `AutoFetch.retryFailed`). A count, not a time: the
 * clock may be set back behind any time kept here.
 */
let onlineCount = 0;

/**
 * The background fetch of one repository. The tabs of its worktrees share one instance (they have
 * the same remotes and remote branches): a remote fetched through one tab is current in all of them,
 * and two of them never fetch at once (two fetches race for the locks of the refs they both update).
 */
export class AutoFetch {
  /** A background fetch is running. */
  running = $state(false);
  /**
   * When a remote was last fetched — in the background, by Fetch all or by a pull —, unix ms. Null:
   * not since the repository's first tab opened.
   */
  fetchedAt = $state<number | null>(null);

  readonly #key: string;
  /** The open tabs of the repository. */
  #members = $state.raw<RepoStore[]>([]);
  /** The last attempt of each remote, by name. Not reactive itself: `#version` tells that it changed. */
  #remotes = new Map<string, RemoteState>();
  #version = $state(0);
  /** No fetch before this (unix ms): the wait after one that was cut short. */
  #notBefore = 0;
  /** The running fetch, and the tab (repo id) it runs through. */
  #round: Promise<void> | null = null;
  #host: string | null = null;

  private constructor(key: string) {
    this.#key = key;
  }

  /** The instance of the repository `repo` belongs to, which `repo` is a tab of from now on. */
  static join(repo: RepoStore): AutoFetch {
    const key = repo.info.commonDir;
    let auto = shared.get(key);
    if (!auto) {
      auto = new AutoFetch(key);
      shared.set(key, auto);
    }
    auto.#add(repo);
    return auto;
  }

  /**
   * The network is back: in every repository, the remotes that failed are due again — also those a
   * fetch is trying right now, should that attempt, begun without a network, fail.
   */
  static retryFailed(): void {
    onlineCount++;
    for (const auto of shared.values()) auto.#retryFailed();
  }

  /** Background fetching was switched off: what runs stops. */
  static stopAll(): void {
    for (const auto of shared.values()) void auto.yield();
  }

  #add(repo: RepoStore): void {
    this.#members = [...this.#members, repo];
  }

  /** The tab of `repo` closed. With the repository's last tab its fetch state goes. */
  leave(repo: RepoStore): void {
    this.#members = this.#members.filter((m) => m !== repo);
    if (this.#members.length === 0 && shared.get(this.#key) === this) shared.delete(this.#key);
  }

  /** An operation of the user's runs in one of the repository's tabs. */
  get busy(): boolean {
    return this.#members.some((m) => m.ops.busy != null);
  }

  /** Whether the tab `host` has a remote to fetch. */
  has(host: RepoStore): boolean {
    return fetchable(host).length > 0;
  }

  /** The last attempt of `remote`, unless it was made with another url. */
  #state(remote: FetchRemote): RemoteState | undefined {
    const st = this.#remotes.get(remote.name);
    return st && st.url === remote.url ? st : undefined;
  }

  /**
   * When `remote` is due (unix ms): at once before its first attempt, else `waitAfter` its last one.
   * At once, too, when that attempt lies ahead of `now`: the clock was set back, and waiting for it
   * to catch up could take hours.
   */
  #nextAt(remote: FetchRemote, now: number): number {
    const st = this.#state(remote);
    return st && st.at <= now ? st.at + waitAfter(st.failures) : 0;
  }

  /** The end of the wait after a fetch that was cut short (none once the clock was set back behind it). */
  #heldUntil(now: number): number {
    return this.#notBefore > now + RESUME_MS ? 0 : this.#notBefore;
  }

  /**
   * When the next background fetch is due (unix ms, possibly past), going by the remotes the tab
   * `host` lists. Null without a remote to fetch.
   */
  dueAt(host: RepoStore): number | null {
    void this.#version;
    const remotes = fetchable(host);
    if (remotes.length === 0) return null;
    const now = Date.now();
    let due = Infinity;
    for (const r of remotes) due = Math.min(due, this.#nextAt(r, now));
    return Math.max(due, this.#heldUntil(now));
  }

  /**
   * The remotes of `host` whose background fetch keeps failing (WARN_FAILURES times in a row by now),
   * with git's message.
   */
  failures(host: RepoStore): FetchFailure[] {
    void this.#version;
    const out: FetchFailure[] = [];
    for (const r of fetchable(host)) {
      const st = this.#state(r);
      if (st && st.failures >= WARN_FAILURES && st.error != null) out.push({ remote: r.name, error: st.error });
    }
    return out;
  }

  /**
   * Fetch through the tab `host`, when a remote is due: that one and, of those that are not waiting
   * after a failure (that wait is the point), the ones due within TOGETHER_MS, one after the other.
   * Nothing while a fetch runs, while an operation of the user's does, or before anything is due.
   * `go` is asked before each remote: once it says no (the window was hidden, another repository's
   * tab came up) the fetch ends with the remote it is at, and the rest stays due. Resolves once the
   * fetch is over; never rejects.
   */
  run(host: RepoStore, go: () => boolean = () => true): Promise<void> {
    if (this.#round) return this.#round;
    if (this.busy || !this.#members.includes(host)) return Promise.resolve();
    const now = Date.now();
    if (now < this.#heldUntil(now)) return Promise.resolve();
    const remotes = fetchable(host);
    if (!remotes.some((r) => this.#nextAt(r, now) <= now)) return Promise.resolve();
    const due = remotes.filter((r) => {
      const at = this.#nextAt(r, now);
      return at <= now || (at <= now + TOGETHER_MS && !this.#state(r)?.failures);
    });
    this.running = true;
    this.#host = host.id;
    const round = this.#fetch(host, due, go).finally(() => {
      this.#round = null;
      this.#host = null;
      this.running = false;
    });
    this.#round = round;
    return round;
  }

  /**
   * One background fetch through `host`: the remotes of `due` in turn, a backend call each, and what
   * each means for the schedule. Never rejects.
   */
  async #fetch(host: RepoStore, due: FetchRemote[], go: () => boolean): Promise<void> {
    let fetched = false;
    let updated = false;
    /** It ended before it got to every remote: those stay due. */
    let cut = false;
    for (const remote of due) {
      // Not on into an operation of the user's, a closed tab, or what `go` rules out.
      if (this.busy || !go() || !this.#members.includes(host)) {
        cut = true;
        break;
      }
      const online = onlineCount;
      let result: RemoteFetch | undefined;
      /** The call itself failed. */
      let failure: string | null = null;
      try {
        const results = await api.fetchBackground(host.id, [remote.name]);
        result = results.find((r) => r.remote === remote.name);
      } catch (e) {
        // Unless its tab closed under it: then it was cut short, not refused.
        if (this.#members.includes(host)) failure = errorMessage(e);
      }
      if (!result && failure == null) {
        // Stopped before it was through (a fetch / pull / push of the user's goes first, the tab closed).
        cut = true;
        break;
      }
      const now = Date.now();
      // A fetch that got as far as the refs went through, whatever git says about one of them (a tag
      // it would have to move, a submodule): backing off would only keep the others from updating.
      const error = result ? (result.updated ? null : result.error) : failure;
      const failures = error == null ? 0 : (this.#state(remote)?.failures ?? 0) + 1;
      // Begun before the network reported back and failed: due again at once, like those that had
      // failed before (`retryFailed`).
      const at = error != null && online !== onlineCount ? 0 : now;
      this.#remotes.set(remote.name, { url: remote.url, at, failures, error });
      if (result && !result.skipped && error == null) fetched = true;
      if (result?.updated) updated = true;
    }
    const now = Date.now();
    if (cut) this.#notBefore = now + RESUME_MS;
    if (fetched) this.fetchedAt = now;
    this.#version++;
    // The watcher reports the moved refs as well; without one (it could not start) this is the reload.
    if (updated) for (const m of this.#members) void m.refresh(['refs']);
  }

  /**
   * A fetch the user ran through the tab `host` went through, for `remotes` (every remote when
   * omitted): they are current, whatever their background fetch last said.
   */
  fetched(host: RepoStore, remotes?: readonly string[]): void {
    const now = Date.now();
    for (const r of fetchable(host)) {
      if (remotes && !remotes.includes(r.name)) continue;
      this.#remotes.set(r.name, { url: r.url, at: now, failures: 0, error: null });
    }
    this.fetchedAt = now;
    this.#version++;
  }

  /**
   * Stop the background fetch; resolves once its git is gone. A fetch / pull / push of the user's
   * calls it before it runs, in the repository: two fetches race for the locks of the refs they both
   * update, and what a pull merges must not move under it. That operation is `busy` by then, which
   * keeps the fetch from going on to its next remote and another from starting.
   */
  async yield(): Promise<void> {
    for (;;) {
      const round = this.#round;
      if (!round) return;
      if (this.#host != null) await api.cancelFetchBackground(this.#host).catch(() => {});
      // Asked again until it has ended: a request that overtook the fetch's own start found nothing
      // to cancel.
      await Promise.race([round, sleep(CANCEL_RETRY_MS)]);
    }
  }

  /** The remotes that failed are due again (their count of failures stays: the next one waits longer). */
  #retryFailed(): void {
    let any = false;
    for (const st of this.#remotes.values()) {
      if (st.failures === 0) continue;
      st.at = 0;
      any = true;
    }
    if (any) this.#version++;
  }
}

/**
 * Keep the repository of the active tab (`active`) fetched while `settings.autoFetch` is on and the
 * window shows (not minimized, hidden or fully covered: nobody looks, and an agent that asks before
 * it signs should not ask then): a fetch starts when one is due — every AUTO_FETCH_MS, and for a tab
 * that comes up with an older (or no) fetch SETTLE_MS later — unless one runs or the user's own
 * operation does. A fetch that runs when the window hides, or when a tab of another repository
 * comes up, finishes the remote it is at (stopped half-way it would start over, leaving its partial
 * pack behind) and leaves the rest for later; one that runs when the setting goes off stops.
 * Whether the machine is online is not asked (a remote may be a folder, and the answer may be
 * wrong): a fetch that cannot connect fails fast and is tried again, at once when the network
 * reports back. Returns the disposer.
 */
export function startAutoFetch(active: () => RepoStore | null): () => void {
  // A window that has the focus shows, whatever the page is told about its visibility.
  const isShown = () => document.visibilityState !== 'hidden' || document.hasFocus();
  let shown = $state(isShown());
  /** Bumped to look at the clock again. */
  let beat = $state(0);

  const look = () => {
    shown = isShown();
    beat++;
  };
  const onOnline = () => {
    AutoFetch.retryFailed();
    look();
  };
  const stop = $effect.root(() => {
    $effect(() => {
      void beat;
      if (!settings.autoFetch || !shown) return;
      const repo = active();
      if (!repo || !repo.ready) return;
      const auto = repo.autoFetch;
      if (auto.running || auto.busy) return;
      const due = auto.dueAt(repo);
      if (due == null) return;
      const wait = Math.max(SETTLE_MS, due - Date.now());
      // Once anything read above changes, this effect runs again and the timer goes. The timer itself
      // only looks at the clock: a fetch that starts sets `running`, and otherwise the bump arms the
      // next look (never a tighter loop than SETTLE_MS, and one that fetches nothing).
      const timer = setTimeout(
        () => {
          // Asked again before each remote: a fetch goes on only for what would let it start.
          const go = () => settings.autoFetch && shown && active()?.autoFetch === auto;
          if (Date.now() >= due) void auto.run(repo, go);
          beat++;
        },
        Math.min(wait, CHECK_MS),
      );
      return () => clearTimeout(timer);
    });

    $effect(() => {
      if (!settings.autoFetch) AutoFetch.stopAll();
    });
  });

  document.addEventListener('visibilitychange', look);
  window.addEventListener('focus', look);
  window.addEventListener('blur', look);
  window.addEventListener('online', onOnline);
  return () => {
    stop();
    document.removeEventListener('visibilitychange', look);
    window.removeEventListener('focus', look);
    window.removeEventListener('blur', look);
    window.removeEventListener('online', onOnline);
  };
}
