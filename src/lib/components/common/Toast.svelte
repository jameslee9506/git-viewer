<!-- Bottom-center pill showing toastState.message after every toast() call. -->
<script lang="ts">
  import { untrack } from 'svelte';
  import { fade } from 'svelte/transition';
  import { toastState } from '../../state/ui.svelte';

  /** Visible time; long messages (errors) get extra reading time, capped. */
  const BASE_MS = 1600;
  const PER_CHAR_MS = 35;
  const MAX_MS = 5000;

  let text = $state('');
  let visible = $state(false);

  $effect(() => {
    if (toastState.seq === 0) return;
    const message = untrack(() => toastState.message);
    text = message;
    visible = true;
    const ms = Math.min(MAX_MS, BASE_MS + Math.max(0, message.length - 40) * PER_CHAR_MS);
    const timer = setTimeout(() => (visible = false), ms);
    return () => clearTimeout(timer);
  });
</script>

{#if visible && text}
  <div class="toast" role="status" aria-live="polite" transition:fade={{ duration: 120 }}>{text}</div>
{/if}

<style>
  .toast {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 24px;
    margin: 0 auto;
    z-index: 1100;
    width: max-content;
    max-width: min(560px, calc(100vw - 48px));
    padding: 5px 14px;
    background: var(--bg-3);
    color: var(--text-0);
    border: 1px solid var(--border-strong);
    border-radius: 14px;
    box-shadow: var(--shadow);
    font-size: var(--fs-sm);
    line-height: 1.4;
    text-align: center;
    white-space: pre-line;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    overflow: hidden;
    pointer-events: none;
  }
</style>
