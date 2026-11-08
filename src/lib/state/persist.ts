// localStorage helpers (WKWebView storage persists across launches).

const PREFIX = 'gv.';

export function loadJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (raw == null) return fallback;
    const parsed = JSON.parse(raw) as T;
    if (fallback && typeof fallback === 'object' && !Array.isArray(fallback) && parsed && typeof parsed === 'object') {
      return { ...fallback, ...parsed };
    }
    return parsed;
  } catch {
    return fallback;
  }
}

export function saveJSON(key: string, value: unknown): void {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    /* quota or private mode: ignore */
  }
}

/** Debounced save, coalescing bursts of changes. */
export function debouncedSaver(key: string, ms = 300): (value: unknown) => void {
  let t: ReturnType<typeof setTimeout> | undefined;
  return (value) => {
    if (t) clearTimeout(t);
    const snapshot = JSON.stringify(value);
    t = setTimeout(() => {
      try {
        localStorage.setItem(PREFIX + key, snapshot);
      } catch {
        /* ignore */
      }
    }, ms);
  };
}
