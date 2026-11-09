<!--
  One line of code: syntax tokens merged with optional change ranges (intra-line diff, kind 1/2).
  Builds one escaped HTML string (a two-pointer sweep over token and range boundaries). The text always
  comes from `text`; tokens only provide colors, so mismatched tokens can never alter the content.
  Bidi control characters are isolated and marked so they cannot reorder the visible code; other control
  characters (a lone CR would become a line break and overflow the fixed-height row) are drawn as
  control pictures (␍ ␀ ␛ …), one character each so offsets stay valid.
  Also used by the blame and file views.
-->
<script lang="ts" module>
  import type { Token } from '../../highlight';

  /** `& < >`, C0 controls except TAB, DEL, U+2028/2029, bidi embeddings/overrides/isolates. */
  const NEEDS_ESCAPE = /[&<>\x00-\x08\x0a-\x1f\x7f\u2028\u2029\u202a-\u202e\u2066-\u2069]/;
  const ESCAPE_ALL = /[&<>\x00-\x08\x0a-\x1f\x7f\u2028\u2029\u202a-\u202e\u2066-\u2069]/g;
  const SAFE_COLOR = /^[\w#(),.%\- ]+$/;
  const openTags = new Map<string, string>();

  function escapeChar(c: string): string {
    if (c === '&') return '&amp;';
    if (c === '<') return '&lt;';
    if (c === '>') return '&gt;';
    const code = c.charCodeAt(0);
    if (code < 0x20) return `<span class="cc">${String.fromCharCode(0x2400 + code)}</span>`;
    if (code === 0x7f) return '<span class="cc">\u2421</span>';
    if (code === 0x2028 || code === 0x2029) return '<span class="cc">\u2424</span>';
    const hex = code.toString(16).toUpperCase().padStart(4, '0');
    return `<span class="bidi" title="U+${hex}">${c}</span>`;
  }

  function escape(s: string): string {
    return NEEDS_ESCAPE.test(s) ? s.replace(ESCAPE_ALL, escapeChar) : s;
  }

  function openTag(color: string, chg: boolean): string {
    const key = chg ? `\u0001${color}` : color;
    let tag = openTags.get(key);
    if (tag === undefined) {
      const style = color && SAFE_COLOR.test(color) ? ` style="color:${color}"` : '';
      tag = chg ? `<span class="chg"${style}>` : style ? `<span${style}>` : '';
      openTags.set(key, tag);
    }
    return tag;
  }

  /** HTML for one line: colored token runs, change ranges wrapped in `.chg`. */
  export function lineHtml(text: string, tokens: Token[] | null, ranges: [number, number][] | null): string {
    const n = text.length;
    const toks = tokens && tokens.length ? tokens : null;
    const rngs = ranges && ranges.length ? ranges : null;
    if (!toks && !rngs) return escape(text);
    let out = '';
    let pos = 0;
    let ti = 0;
    let tokEnd = toks ? 0 : n;
    let color = '';
    let ri = 0;
    let runStart = 0;
    let runColor = '';
    let runChg = false;
    const flush = (end: number) => {
      if (end <= runStart) return;
      const tag = openTag(runColor, runChg);
      const seg = escape(text.slice(runStart, end));
      out += tag ? `${tag}${seg}</span>` : seg;
    };
    while (pos < n) {
      if (toks) {
        while (tokEnd <= pos) {
          if (ti < toks.length) {
            tokEnd += toks[ti][0].length;
            color = toks[ti][1];
            ti++;
          } else {
            tokEnd = n;
            color = '';
          }
        }
      }
      let end = tokEnd < n ? tokEnd : n;
      let chg = false;
      if (rngs) {
        while (ri < rngs.length && rngs[ri][1] <= pos) ri++;
        if (ri < rngs.length) {
          const [s, e] = rngs[ri];
          if (s <= pos) {
            chg = true;
            if (e < end) end = e;
          } else if (s < end) end = s;
        }
      }
      if (color !== runColor || chg !== runChg) {
        flush(pos);
        runStart = pos;
        runColor = color;
        runChg = chg;
      }
      pos = end;
    }
    flush(n);
    return out;
  }
</script>

<script lang="ts">
  interface Props {
    text: string;
    tokens: Token[] | null;
    ranges: [number, number][] | null;
    /** 1 = added side, 2 = deleted side (selects the change-range color). */
    kind?: 0 | 1 | 2;
  }

  let { text, tokens, ranges, kind = 0 }: Props = $props();

  const html = $derived(
    lineHtml(text.charCodeAt(text.length - 1) === 13 ? text.slice(0, -1) : text, tokens, ranges),
  );
</script>

<span class="dlt" class:add={kind === 1} class:del={kind === 2}>{@html html}</span>

<style>
  .dlt {
    color: var(--shiki-foreground);
    white-space: pre;
  }
  .dlt :global(.chg) {
    border-radius: 2px;
    background: var(--bg-match-current);
  }
  .dlt.add :global(.chg) {
    background: var(--add-bg-strong);
  }
  .dlt.del :global(.chg) {
    background: var(--del-bg-strong);
  }
  .dlt :global(.cc) {
    color: var(--shiki-token-comment);
  }
  .dlt :global(.bidi) {
    unicode-bidi: isolate;
  }
  .dlt :global(.bidi)::before {
    content: '⇄';
    color: var(--st-conflict);
  }
</style>
