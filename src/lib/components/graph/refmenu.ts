// The menu of a ref label in the graph (RefBadges, RefPopover): what can be done with the branch or
// the tag the label stands for — check it out, delete it (here and on its remotes), copy its name,
// show it alone. The other labels (a detached HEAD, a stash, a worktree) have none: the row's menu
// opens on them.
import CloudOff from '@lucide/svelte/icons/cloud-off';
import Copy from '@lucide/svelte/icons/copy';
import GitBranch from '@lucide/svelte/icons/git-branch';
import Target from '@lucide/svelte/icons/target';
import Trash from '@lucide/svelte/icons/trash';
import type { RepoStore } from '../../state/repo.svelte';
import { toast, type MenuEntry } from '../../state/ui.svelte';
import type { CheckoutTarget, RefLabelKind } from '../../types';
import { copyText } from '../../util/format';

/** A ref label: its ref and, for a local branch, the remote branches shown in the same label (full names). */
export interface LabelRef {
  kind: RefLabelKind;
  fullName: string;
  remotes: readonly string[];
}

/** A remote to delete from. */
interface RemoteTarget {
  remote: string;
  run: () => void;
}

/** With more remotes to delete from than this, one entry opens them. */
const MAX_REMOTES = 2;
const SEP: MenuEntry = { separator: true };

/** Whether labels of this kind have a menu of their own: branches and tags. */
export function hasRefMenu(kind: RefLabelKind): boolean {
  return kind === 'local' || kind === 'remote' || kind === 'tag';
}

/** `Delete from <remote>…` for each remote; past MAX_REMOTES of them, one entry that opens them. */
function fromRemotes(targets: RemoteTarget[], off: boolean): MenuEntry[] {
  if (targets.length <= MAX_REMOTES) {
    return targets.map((t) => ({ label: `Delete from ${t.remote}…`, icon: CloudOff, disabled: off, action: t.run }));
  }
  return [
    {
      label: 'Delete from',
      icon: CloudOff,
      disabled: off,
      children: targets.map((t) => ({ label: `${t.remote}…`, action: t.run })),
    },
  ];
}

/**
 * The entries for `label`: Checkout (a branch; none in a bare repo) | Delete… (the branch or the tag
 * here), Delete from <remote>… (a remote branch on its remote, a tag on every remote — remotes with
 * a URL: git would take another name for a path) | Copy name | Solo. Empty for a label without a
 * menu, and for a ref that the store's snapshot no longer holds. Every deletion asks first
 * (`WriteOps`).
 */
export function refMenu(repo: RepoStore, label: LabelRef): MenuEntry[] {
  const refs = repo.refs;
  if (!refs) return [];
  const st = repo.status;
  const ops = repo.ops;
  const busy = !!ops.busy;
  // Like the row menu's: a checkout waits for a merge / rebase / … in progress to end.
  const held = busy || !!st?.operation;
  const checkout = (target: CheckoutTarget, taken: boolean): MenuEntry[] =>
    st
      ? [{ label: 'Checkout', icon: GitBranch, disabled: held || taken, action: () => void ops.checkout(target) }, SEP]
      : [];
  /** A remote to delete from: one with a URL. Remote branches may outlive their remote's settings. */
  const configured = (remote: string): boolean => refs.remoteList.some((r) => r.name === remote && r.url != null);

  const items: MenuEntry[] = [];
  /** What Copy name copies. */
  let name: string;
  switch (label.kind) {
    case 'local': {
      const b = refs.locals.find((l) => l.fullName === label.fullName);
      if (!b) return [];
      // Checked out, here or in another worktree: nothing to switch to, and git would not delete it.
      const taken = b.isHead || !!b.worktree;
      const remotes: RemoteTarget[] = [];
      for (const full of label.remotes) {
        const r = refs.remotes.find((x) => x.fullName === full);
        if (r && configured(r.remote)) remotes.push({ remote: r.remote, run: () => void ops.deleteRemoteBranch(r) });
      }
      items.push(
        ...checkout({ kind: 'branch', name: b.name }, taken),
        { label: 'Delete…', icon: Trash, disabled: busy || taken, action: () => void ops.deleteBranch(b) },
        ...fromRemotes(remotes, busy),
      );
      name = b.name;
      break;
    }
    case 'remote': {
      const b = refs.remotes.find((r) => r.fullName === label.fullName);
      if (!b) return [];
      // As in the sidebar: its local branch (the branch of its name that tracks it), else a new one
      // tracking it. A branch of that name with another upstream stands in the way of both.
      const local = refs.locals.find((l) => l.name === b.short);
      const target: CheckoutTarget = local
        ? { kind: 'branch', name: local.name }
        : { kind: 'track', name: b.short, upstream: b.fullName };
      const taken = !!local && (local.upstream !== b.name || local.isHead || !!local.worktree);
      const remotes: RemoteTarget[] = configured(b.remote)
        ? [{ remote: b.remote, run: () => void ops.deleteRemoteBranch(b) }]
        : [];
      items.push(...checkout(target, taken), ...fromRemotes(remotes, busy));
      name = b.name;
      break;
    }
    case 'tag': {
      const t = refs.tags.find((x) => x.fullName === label.fullName);
      if (!t) return [];
      // Which remotes hold the tag is not known here: every configured one is offered.
      const remotes = refs.remoteList
        .filter((r) => r.url != null)
        .map((r): RemoteTarget => ({ remote: r.name, run: () => void ops.deleteRemoteTag(t.name, r.name) }));
      items.push(
        { label: 'Delete…', icon: Trash, disabled: busy, action: () => void ops.deleteTag(t) },
        ...fromRemotes(remotes, busy),
      );
      name = t.name;
      break;
    }
    default:
      return [];
  }
  const soloed = repo.onlyRefs.includes(label.fullName);
  items.push(
    SEP,
    { label: 'Copy name', icon: Copy, action: () => void copyText(name).then(() => toast('Copied')) },
    SEP,
    { label: soloed ? 'Unsolo' : 'Solo', icon: Target, action: () => repo.solo(label.fullName) },
  );
  return items;
}
