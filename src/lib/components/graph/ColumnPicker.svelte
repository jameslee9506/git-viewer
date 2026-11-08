<!--
  Column picker hanging under the graph header's gear (render it inside the `.gv` root): one row per
  optional column with a check while it is on. Every row writes `settings` directly and the picker
  stays open for further toggles; it closes on Esc and on pointerdown outside itself and its anchor.
-->
<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import { settings } from '../../state/settings.svelte';
  import { isComposing } from '../../util/keys';
  import { COLUMN_TOGGLES } from './columns';

  interface Props {
    /** The gear button: pointerdowns on it are not "outside" (its click toggles the picker). */
    anchor?: HTMLElement | null;
    onclose: () => void;
  }

  let { anchor = null, onclose }: Props = $props();

  let el: HTMLDivElement | undefined = $state();

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

<div class="pop" bind:this={el} role="group" aria-label="Columns">
  {#each COLUMN_TOGGLES as t (t.key)}
    <!-- mousedown is cancelled so toggling never moves keyboard focus off the graph -->
    <button
      class="item"
      class:on={settings[t.key]}
      aria-pressed={settings[t.key]}
      onmousedown={(e) => e.preventDefault()}
      onclick={() => (settings[t.key] = !settings[t.key])}
    >
      <span class="icon"><t.icon size={14} strokeWidth={1.75} /></span>
      <span class="label">{t.label}</span>
      <span class="check">
        {#if settings[t.key]}<Check size={12} strokeWidth={2.25} />{/if}
      </span>
    </button>
  {/each}
</div>

<style>
  .pop {
    position: absolute;
    top: calc(var(--header-h) + 4px);
    right: 4px;
    z-index: 7;
    min-width: 164px;
    padding: 4px;
    background: var(--bg-float);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 24px;
    padding: 0 8px;
    border-radius: var(--radius-sm);
    color: var(--text-1);
    text-align: left;
  }
  .item:hover {
    background: var(--bg-hover);
  }
  .item.on {
    color: var(--text-0);
  }

  .icon {
    display: inline-flex;
    width: 14px;
    flex: none;
    color: var(--text-2);
  }
  .item.on .icon {
    color: var(--text-1);
  }

  .label {
    flex: 1 1 auto;
  }

  .check {
    display: inline-flex;
    justify-content: center;
    width: 12px;
    flex: none;
    color: var(--accent);
  }
</style>
