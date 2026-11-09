<!--
  Author avatar: the identicon (util/identicon.ts) in the author's hue on a tint of it, clipped to a
  circle; the name and email in its title. No network avatars.
-->
<script lang="ts">
  import { AVATAR_R, identicon } from '../../util/identicon';

  interface Props {
    name: string;
    email: string;
    /** Diameter in px; a multiple of 16 keeps the glyph's cells on whole pixels. */
    size?: number;
  }

  let { name, email, size = 16 }: Props = $props();

  const id = $derived(identicon(name, email));
</script>

<span
  class="avatar"
  style:width="{size}px"
  style:height="{size}px"
  style:--c="var(--lane-{id.hue})"
  title={email ? `${name} <${email}>` : name}
>
  <svg width={size} height={size} viewBox="{-AVATAR_R} {-AVATAR_R} {2 * AVATAR_R} {2 * AVATAR_R}" aria-hidden="true">
    <path d={id.path} />
  </svg>
</span>

<style>
  .avatar {
    display: inline-flex;
    flex: none;
    overflow: hidden;
    border-radius: 50%;
    background: color-mix(in srgb, var(--c) 25%, transparent);
  }
  svg {
    display: block;
    shape-rendering: crispEdges;
  }
  path {
    fill: var(--c);
  }
</style>
