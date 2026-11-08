<!-- CSS-only rotating ring. -->
<script lang="ts">
  interface Props {
    /** Diameter in px. */
    size?: number;
  }

  let { size = 12 }: Props = $props();
</script>

<span class="spinner" style:--spinner-size="{size}px" aria-hidden="true"></span>

<style>
  .spinner {
    display: inline-block;
    flex: none;
    width: var(--spinner-size);
    height: var(--spinner-size);
    border: max(1.5px, calc(var(--spinner-size) / 9)) solid var(--border-strong);
    border-top-color: var(--accent);
    border-radius: 50%;
    vertical-align: middle;
    animation: spin 0.7s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
