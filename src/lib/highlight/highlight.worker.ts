// Syntax highlighting worker: shiki core + Oniguruma (WASM) engine + CSS-variables theme. Grammars load lazily, one
// chunk each. Documents are tokenized newest-first in short time slices, so a new request never waits behind an
// older document (older ones resume afterwards unless the client cancels them).
import { createCssVariablesTheme, createHighlighterCore } from 'shiki/core';
import type { GrammarState, HighlighterCore, LanguageRegistration, ThemedToken } from 'shiki/core';
import { createOnigurumaEngine } from 'shiki/engine/oniguruma';
import { bundledLanguages, bundledLanguagesInfo } from 'shiki/langs';
import type { BundledLanguage } from 'shiki/langs';

/** Tokenize `lines` as one document; the grammar restarts at every index in `resets`. */
export interface HighlightJob {
  id: number;
  lang: string;
  lines: string[];
  /** Sorted, unique, each inside (0, lines.length). */
  resets: number[];
}

export type ToWorker = HighlightJob | { cancel: number };

/**
 * Packed tokens of a document. Line i owns tokens `starts[i] .. starts[i + 1]`; a token ends at `ends[k]` (exclusive)
 * and starts where the previous token of its line ended (0 for the first). The tokens of a line cover it exactly,
 * minus one trailing '\r'.
 */
export interface PackedTokens {
  starts: Uint32Array;
  ends: Uint32Array;
  colors: Uint8Array;
  /** Color strings by index; index 0 is '' (plain). */
  palette: string[];
}

/** `busy`: the worker started (or resumed) the job with that id. `data` null = unsupported / failed / too slow. */
export type FromWorker = { id: number; data: PackedTokens | null } | { busy: number };

interface WorkerScope {
  onmessage: ((e: MessageEvent<ToWorker>) => void) | null;
  postMessage(message: FromWorker, transfer: Transferable[]): void;
}

interface Task {
  id: number;
  requested: string;
  /** Lines without a trailing '\r'. */
  text: string[];
  resets: number[];
  /** Resolved language: undefined until prepared, null when it cannot be highlighted. */
  lang: string | null | undefined;
  /** Grammar the carried `state` belongs to (a reload invalidates it). */
  grammar: unknown;
  state: GrammarState | undefined;
  pos: number;
  nextReset: number;
  starts: Uint32Array<ArrayBuffer>;
  ends: number[];
  colors: number[];
  spent: number;
}

const THEME = 'gv';
const PLAIN = 'var(--shiki-foreground)';
/** Lines per codeToTokens call; grammar state is carried across calls. */
const CHUNK_LINES = 128;
/** Work per slice before yielding to the message loop (new jobs, cancels). */
const SLICE_MS = 12;
/** A job that needs more work than this resolves null (below the client's 5s watchdog). */
const TASK_BUDGET_MS = 4000;
/** Longer lines become one plain token. */
const MAX_LINE_LENGTH = 2000;
/** Per-line tokenizer time limit; the rest of a slow line keeps the color reached so far. */
const LINE_TIME_LIMIT_MS = 500;
/** Leading lines scanned for C++ / Objective-C markers in a `c` document. */
const C_HINT_LINES = 2000;
/** Embedded-language hints: at most this many grammars per document, from lines up to this long. */
const MAX_EMBEDDED = 8;
const MAX_HINT_LINE = 300;

const EMBED_HINTS = [
  /^\s*(?:`{3,}|~{3,})\s*\{?\s*\.?([\w#+.-]+)/, // ```ts, ~~~ {.python}
  /\blang(?:uage)?\s*=\s*["']?([\w#+.-]+)/i, // <style lang="scss">, <syntaxhighlight lang="c">
  /^\s*\[source,\s*([\w#+.-]+)/, // asciidoc [source,ruby]
  /\\begin\{minted\}(?:\[[^\]]*\])?\{([\w#+.-]+)\}/, // latex minted
];
const OBJC_HINT = /^\s*(?:@(?:interface|implementation|protocol|property|end)\b|#\s*import\s*[<"])/;
const CPP_HINT =
  /^\s*(?:template\s*<|namespace\s+\w|class\s+\w+\s*(?:final\s*)?[:{]|(?:public|private|protected)\s*:|#\s*include\s*<(?:iostream|string|vector|map|set|memory|algorithm|utility|functional|unordered_map|unordered_set|array|optional|tuple|thread|mutex|chrono|sstream|fstream|cstdint|cstdio|cstdlib|cstring|cassert|cmath)>)|\bstd::\w|\b(?:static|reinterpret|dynamic|const)_cast\s*</;

const scope = self as unknown as WorkerScope;
const tasks: Task[] = [];
const grammars = new Map<string, Promise<LanguageRegistration | null>>();
const palette: string[] = [''];
const paletteIndex = new Map<string, number>([
  ['', 0],
  [PLAIN, 0],
]);
let canonicalIds: Map<string, string> | null = null;
let core: Promise<HighlighterCore> | null = null;
let running = false;
let announced = 0;

const tick = new MessageChannel();
const wakers: (() => void)[] = [];
tick.port1.onmessage = () => wakers.shift()?.();

scope.onmessage = ({ data }) => {
  if ('cancel' in data) {
    const i = tasks.findIndex((t) => t.id === data.cancel);
    if (i >= 0) tasks.splice(i, 1);
    return;
  }
  tasks.push(createTask(data));
  if (!running) void run();
};

function createTask(job: HighlightJob): Task {
  const text = job.lines.map((l) => (l.charCodeAt(l.length - 1) === 13 ? l.slice(0, -1) : l));
  return {
    id: job.id,
    requested: job.lang,
    text,
    resets: job.resets,
    lang: undefined,
    grammar: null,
    state: undefined,
    pos: 0,
    nextReset: 0,
    starts: new Uint32Array(text.length + 1),
    ends: [],
    colors: [],
    spent: 0,
  };
}

/** Zero-delay yield (setTimeout would be clamped to 4ms once nested). */
function yieldToLoop(): Promise<void> {
  return new Promise((resolve) => {
    wakers.push(resolve);
    tick.port2.postMessage(0);
  });
}

async function run(): Promise<void> {
  running = true;
  try {
    while (tasks.length > 0) {
      const task = tasks[tasks.length - 1];
      if (task.id !== announced) {
        announced = task.id;
        scope.postMessage({ busy: task.id }, []);
      }
      const t0 = performance.now();
      let done: boolean;
      try {
        done = await advance(task, t0);
      } catch (err) {
        console.warn('[highlight]', err);
        task.lang = null;
        done = true;
      }
      task.spent += performance.now() - t0;
      if (!done && task.spent > TASK_BUDGET_MS) {
        task.lang = null;
        done = true;
      }
      const i = tasks.indexOf(task);
      if (done && i >= 0) {
        tasks.splice(i, 1);
        reply(task);
      }
      await yieldToLoop();
    }
  } finally {
    running = false;
  }
}

/** One slice of work on `task`; true when it is finished (successfully or not). */
async function advance(task: Task, t0: number): Promise<boolean> {
  if (task.lang === undefined) {
    task.lang = await prepare(task.requested, task.text);
    return task.lang === null;
  }
  if (task.lang === null) return true;
  const h = await highlighter();
  const lang = task.lang;
  const n = task.text.length;
  while (task.pos < n) {
    while (task.nextReset < task.resets.length && task.resets[task.nextReset] <= task.pos) {
      if (task.resets[task.nextReset] === task.pos) task.state = undefined;
      task.nextReset++;
    }
    const grammar = h.getLanguage(lang);
    if (task.state && task.grammar !== grammar) rewind(task);
    task.grammar = grammar;
    const limit = task.nextReset < task.resets.length ? task.resets[task.nextReset] : n;
    const end = Math.min(limit, task.pos + CHUNK_LINES);
    const result = h.codeToTokens(task.text.slice(task.pos, end).join('\n'), {
      lang,
      theme: THEME,
      tokenizeMaxLineLength: MAX_LINE_LENGTH,
      tokenizeTimeLimit: LINE_TIME_LIMIT_MS,
      grammarState: task.state,
    });
    task.state = result.grammarState;
    pack(task, result.tokens, task.pos, end);
    task.pos = end;
    if (performance.now() - t0 >= SLICE_MS) break;
  }
  return task.pos >= n;
}

/**
 * Start over: another job loaded a language that made shiki rebuild this grammar (lazy embeds, injections), so the
 * carried grammar state belongs to a discarded grammar.
 */
function rewind(task: Task): void {
  task.pos = 0;
  task.nextReset = 0;
  task.state = undefined;
  task.ends.length = 0;
  task.colors.length = 0;
}

/** Converts one chunk (lines `from..to`) to packed tokens: plain = 0, adjacent same-color tokens merged. */
function pack(task: Task, rows: ThemedToken[][], from: number, to: number): void {
  const { text, starts, ends, colors } = task;
  const exact = rows.length === to - from;
  let base = 0;
  for (let i = from; i < to; i++) {
    const line = text[i];
    const first = ends.length;
    starts[i] = first;
    let pos = 0;
    if (exact) {
      for (const t of rows[i - from]) {
        const a = t.offset - base;
        const z = Math.min(a + t.content.length, line.length);
        if (z <= pos) continue;
        if (a > pos) emit(ends, colors, first, a, 0);
        pos = emit(ends, colors, first, z, colorIndex(t.color));
      }
    }
    if (pos < line.length) emit(ends, colors, first, line.length, 0);
    base += line.length + 1;
  }
}

function emit(ends: number[], colors: number[], first: number, end: number, color: number): number {
  const last = ends.length - 1;
  if (last >= first && colors[last] === color) ends[last] = end;
  else {
    ends.push(end);
    colors.push(color);
  }
  return end;
}

function colorIndex(color: string | undefined): number {
  if (!color) return 0;
  let i = paletteIndex.get(color);
  if (i === undefined) {
    i = palette.length < 256 ? palette.push(color) - 1 : 0;
    paletteIndex.set(color, i);
  }
  return i;
}

function reply(task: Task): void {
  if (!task.lang || task.pos < task.text.length) {
    scope.postMessage({ id: task.id, data: null }, []);
    return;
  }
  const starts = task.starts;
  starts[task.text.length] = task.ends.length;
  const ends = Uint32Array.from(task.ends);
  const colors = Uint8Array.from(task.colors);
  scope.postMessage({ id: task.id, data: { starts, ends, colors, palette: palette.slice() } }, [
    starts.buffer,
    ends.buffer,
    colors.buffer,
  ]);
}

function highlighter(): Promise<HighlighterCore> {
  core ??= createCore();
  return core;
}

async function createCore(): Promise<HighlighterCore> {
  const themes = [
    createCssVariablesTheme({
      name: THEME,
      variablePrefix: '--shiki-',
      fontStyle: true,
      // Diff / markup scopes use variables tokens.css does not define: fall back to the diff colors.
      variableDefaults: {
        'token-inserted': 'var(--add-fg)',
        'token-deleted': 'var(--del-fg)',
        'token-changed': 'var(--st-modified)',
      },
    }),
  ];
  try {
    return await createHighlighterCore({ themes, langs: [], engine: createOnigurumaEngine(import('shiki/wasm')) });
  } catch (err) {
    // The inlined WASM needs Response.bytes() (WebKit 18+); older webviews fall back to the JS regex engine.
    console.warn('[highlight] oniguruma unavailable, using the JavaScript engine', err);
    const { createJavaScriptRegexEngine } = await import('shiki/engine/javascript');
    return createHighlighterCore({ themes, langs: [], engine: createJavaScriptRegexEngine({ forgiving: true }) });
  }
}

/** Resolves the language for a document and loads its grammar (plus embedded ones it uses); null if unsupported. */
async function prepare(requested: string, text: string[]): Promise<string | null> {
  const id = canonical(requested);
  if (!id) return null;
  const h = await highlighter();
  const lang = id === 'c' ? cDialect(text) : id;
  const reg = await loadGrammar(h, lang);
  if (!reg) return null;
  if (reg.embeddedLangsLazy?.length) {
    await Promise.all(embeddedLanguages(text, reg.embeddedLangsLazy).map((l) => loadGrammar(h, l)));
  }
  return lang;
}

function canonical(lang: string): string | null {
  if (!canonicalIds) {
    canonicalIds = new Map();
    for (const info of bundledLanguagesInfo) {
      canonicalIds.set(info.id, info.id);
      for (const alias of info.aliases ?? []) canonicalIds.set(alias, info.id);
    }
  }
  return canonicalIds.get(lang.toLowerCase()) ?? null;
}

function loadGrammar(h: HighlighterCore, id: string): Promise<LanguageRegistration | null> {
  let p = grammars.get(id);
  if (!p) {
    p = bundledLanguages[id as BundledLanguage]()
      .then(async ({ default: regs }) => {
        await h.loadLanguage(regs);
        return regs.find((r) => r.name === id) ?? null;
      })
      .catch((err: unknown) => {
        console.warn('[highlight] grammar', id, err);
        return null;
      });
    grammars.set(id, p);
  }
  return p;
}

/** Lazily embedded languages a document uses (fenced code, `lang="…"`), limited to those its grammar can embed. */
function embeddedLanguages(text: string[], lazy: string[]): string[] {
  const allowed = new Set(lazy);
  const found = new Set<string>();
  for (let i = 0; i < text.length && found.size < MAX_EMBEDDED; i++) {
    const line = text[i];
    if (line.length > MAX_HINT_LINE) continue;
    for (const re of EMBED_HINTS) {
      const m = re.exec(line);
      const id = m ? canonical(m[1]) : null;
      if (id && allowed.has(id)) found.add(id);
    }
  }
  return [...found];
}

/** `.h` maps to C; switch to C++ or Objective-C when the content says so. */
function cDialect(text: string[]): string {
  const n = Math.min(text.length, C_HINT_LINES);
  for (let i = 0; i < n; i++) {
    const line = text[i];
    if (OBJC_HINT.test(line)) return 'objective-c';
    if (CPP_HINT.test(line)) return 'cpp';
  }
  return 'c';
}
