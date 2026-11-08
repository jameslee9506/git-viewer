<!-- Initials circle tinted with a lane color picked by a hash of the email (no network avatars). -->
<script lang="ts">
  import { initials } from '../../util/format';

  interface Props {
    name: string;
    email: string;
    size?: number;
  }

  let { name, email, size = 18 }: Props = $props();

  /** FNV-1a over the normalized email (name when there is none) → lane color 0..7. */
  const lane = $derived.by(() => {
    const key = email.trim().toLowerCase() || name.trim().toLowerCase();
    let h = 0x811c9dc5;
    for (let i = 0; i < key.length; i++) {
      h ^= key.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    return (h >>> 0) % 8;
  });
</script>

<span
  class="avatar"
  style:width="{size}px"
  style:height="{size}px"
  style:font-size="{Math.max(8, Math.round(size * 0.42))}px"
  style:--c="var(--lane-{lane})"
  title={email ? `${name} <${email}>` : name}>{initials(name)}</span
>

<style>
  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    border-radius: 50%;
    background: color-mix(in srgb, var(--c) 25%, transparent);
    color: var(--text-0);
    font-weight: 600;
    letter-spacing: 0.02em;
    line-height: 1;
    overflow: hidden;
    white-space: nowrap;
  }
</style>
