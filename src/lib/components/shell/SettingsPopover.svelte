<!--
  Settings popover hanging under the tab bar's gear (render it inside a `position: relative` wrapper).
  Compact rows with tiny lowercase labels; every control writes `settings` directly.
  Closes on Esc and on pointerdown outside itself and its anchor.
-->
<script module lang="ts">
  import { settings } from '../../state/settings.svelte';

  export const ZOOM_MIN = 0.7;
  export const ZOOM_MAX = 1.6;

  /** Zoom factor clamped to [ZOOM_MIN, ZOOM_MAX]; 1 for non-numbers. */
  export function clampZoom(z: number): number {
    return Number.isFinite(z) ? Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z)) : 1;
  }

  /** Zoom in / out by 10% (`dir` 1 / -1) or back to 100% (`dir` 0). Returns the new factor. */
  export function stepZoom(dir: 1 | -1 | 0): number {
    const z = dir === 0 ? 1 : clampZoom(Math.round(clampZoom(settings.zoom) * 10 + dir) / 10);
    settings.zoom = z;
    return z;
  }
</script>

<script lang="ts">
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import Archive from '@lucide/svelte/icons/archive';
  import Cloud from '@lucide/svelte/icons/cloud';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import Minus from '@lucide/svelte/icons/minus';
  import Monitor from '@lucide/svelte/icons/monitor';
  import Moon from '@lucide/svelte/icons/moon';
  import Palette from '@lucide/svelte/icons/palette';
  import Pin from '@lucide/svelte/icons/pin';
  import Plus from '@lucide/svelte/icons/plus';
  import Space from '@lucide/svelte/icons/space';
  import Sun from '@lucide/svelte/icons/sun';
  import Tag from '@lucide/svelte/icons/tag';
  import { api, errorMessage } from '../../api';
  import type { Settings } from '../../state/settings.svelte';
  import { toast } from '../../state/ui.svelte';
  import { isComposing } from '../../util/keys';
  import { COLUMN_TOGGLES } from '../graph/columns';

  interface Props {
    /** Element the popover hangs from (the gear button): pointerdowns on it are not "outside". */
    anchor?: HTMLElement | null;
    onclose: () => void;
  }

  let { anchor = null, onclose }: Props = $props();

  type BoolKey = { [K in keyof Settings]: Settings[K] extends boolean ? K : never }[keyof Settings];

  interface Toggle {
    key: BoolKey;
    icon: typeof Cloud;
    title: string;
  }

  const GRAPH: Toggle[] = [
    { key: 'showRemotes', icon: Cloud, title: 'remotes' },
    { key: 'showTags', icon: Tag, title: 'tags' },
    { key: 'showStashes', icon: Archive, title: 'stashes' },
    { key: 'pinHeadLane', icon: Pin, title: 'pin HEAD lane' },
    { key: 'showMergesMuted', icon: GitMerge, title: 'muted merges' },
  ];

  /** The graph's optional columns, as the graph header's picker lists them. */
  const COLUMNS: Toggle[] = COLUMN_TOGGLES.map((t) => ({ key: t.key, icon: t.icon, title: t.label.toLowerCase() }));

  const COMMITS = [
    [10000, '10k'],
    [50000, '50k'],
    [200000, '200k'],
    [0, 'all'],
  ] as const;

  const CONTEXT = [3, 10, 25] as const;
  const THEMES = [
    { value: 'system', icon: Monitor, label: 'System theme' },
    { value: 'light', icon: Sun, label: 'Light theme' },
    { value: 'dark', icon: Moon, label: 'Dark theme' },
  ] as const;

  let el: HTMLDivElement | undefined = $state();

  const MIN_PCT = Math.round(ZOOM_MIN * 100);
  const MAX_PCT = Math.round(ZOOM_MAX * 100);

  const pct = $derived(Math.round(clampZoom(settings.zoom) * 100));

  async function openUserCss() {
    try {
      const { path } = await api.userCss(true);
      await revealItemInDir(path);
      onclose();
    } catch (e) {
      toast(errorMessage(e));
    }
  }

  $effect(() => {
    const onDown = (e: PointerEvent) => {
      const t = e.target;
      if (t instanceof Node && (el?.contains(t) || anchor?.contains(t))) return;
      onclose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.code !== 'Escape' || isComposing(e)) return;
      e.preventDefault();
      e.stopPropagation();
      onclose();
    };
    window.addEventListener('pointerdown', onDown, true);
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('keydown', onKey, true);
    };
  });
</script>

{#snippet toggles(list: Toggle[])}
  {#each list as t (t.key)}
    <button
      class:on={settings[t.key]}
      title={t.title}
      aria-pressed={settings[t.key]}
      onclick={() => (settings[t.key] = !settings[t.key])}
    >
      <t.icon size={13} strokeWidth={1.75} />
    </button>
  {/each}
{/snippet}

<div class="pop" bind:this={el} role="dialog" aria-label="Settings">
  <span class="lbl">theme</span>
  <div class="seg" role="group" aria-label="Theme">
    {#each THEMES as theme (theme.value)}
      <button
        class:on={settings.theme === theme.value}
        title={theme.label}
        aria-label={theme.label}
        aria-pressed={settings.theme === theme.value}
        onclick={() => (settings.theme = theme.value)}
      >
        <theme.icon size={13} strokeWidth={1.75} />
      </button>
    {/each}
  </div>

  <span class="lbl">date</span>
  <div class="seg">
    <button class:on={settings.dateFormat === 'relative'} onclick={() => (settings.dateFormat = 'relative')}>
      rel
    </button>
    <button class:on={settings.dateFormat === 'absolute'} onclick={() => (settings.dateFormat = 'absolute')}>
      abs
    </button>
  </div>

  <span class="lbl">order</span>
  <div class="seg">
    <button class:on={settings.graphOrder === 'date'} onclick={() => (settings.graphOrder = 'date')}>date</button>
    <button class:on={settings.graphOrder === 'topo'} onclick={() => (settings.graphOrder = 'topo')}>topo</button>
  </div>

  <span class="lbl">commits</span>
  <div class="seg">
    {#each COMMITS as [n, label] (n)}
      <button class:on={settings.maxCommits === n} onclick={() => (settings.maxCommits = n)}>{label}</button>
    {/each}
  </div>

  <span class="lbl">graph</span>
  <div class="seg">{@render toggles(GRAPH)}</div>

  <span class="lbl">columns</span>
  <div class="seg">{@render toggles(COLUMNS)}</div>

  <span class="lbl">diff</span>
  <div class="ctl">
    <div class="seg">
      <button class:on={settings.diffMode === 'unified'} onclick={() => (settings.diffMode = 'unified')}>
        unified
      </button>
      <button class:on={settings.diffMode === 'split'} onclick={() => (settings.diffMode = 'split')}>split</button>
    </div>
    <div class="seg">
      <button
        class:on={settings.ignoreWhitespace}
        title="ignore whitespace"
        aria-pressed={settings.ignoreWhitespace}
        onclick={() => (settings.ignoreWhitespace = !settings.ignoreWhitespace)}
      >
        <Space size={13} strokeWidth={1.75} />
      </button>
    </div>
  </div>

  <span class="lbl">context</span>
  <div class="seg">
    {#each CONTEXT as n (n)}
      <button class:on={settings.contextLines === n} onclick={() => (settings.contextLines = n)}>{n}</button>
    {/each}
  </div>

  <span class="lbl">zoom</span>
  <div class="ctl">
    <button class="icon-btn" title="zoom out" disabled={pct <= MIN_PCT} onclick={() => stepZoom(-1)}>
      <Minus size={12} strokeWidth={1.75} />
    </button>
    <button class="pct mono" title="actual size" onclick={() => stepZoom(0)}>{pct}%</button>
    <button class="icon-btn" title="zoom in" disabled={pct >= MAX_PCT} onclick={() => stepZoom(1)}>
      <Plus size={12} strokeWidth={1.75} />
    </button>
  </div>

  <span class="lbl">style</span>
  <div class="ctl">
    <button class="chip" title="reveal user.css" onclick={openUserCss}>
      <Palette size={13} strokeWidth={1.75} />
      <span class="mono">user.css</span>
    </button>
  </div>
</div>

<style>
  .pop {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 50;
    display: grid;
    grid-template-columns: auto auto;
    justify-content: start;
    align-items: center;
    gap: 6px 14px;
    padding: 10px 12px;
    background: var(--bg-float);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    white-space: nowrap;
  }

  .lbl {
    font-size: var(--fs-xs);
    color: var(--text-2);
  }

  .ctl {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .pct {
    min-width: 40px;
    height: 20px;
    padding: 0 4px;
    border-radius: 3px;
    color: var(--text-1);
    text-align: center;
    font-size: var(--fs-xs);
  }
  .pct:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    padding: 0 8px;
    border-radius: var(--radius-sm);
    background: var(--bg-3);
    color: var(--text-1);
  }
  .chip:hover {
    color: var(--text-0);
    background: var(--bg-hover);
  }
  .chip .mono {
    font-size: var(--fs-xs);
  }
</style>
