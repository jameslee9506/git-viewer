// The optional graph columns, in column order, as the header's picker, the column context menu and the
// settings popover list them. Each key is a boolean setting.
import Clock from '@lucide/svelte/icons/clock';
import GitBranch from '@lucide/svelte/icons/git-branch';
import Hash from '@lucide/svelte/icons/hash';
import User from '@lucide/svelte/icons/user';

export type ColumnKey = 'refsColumn' | 'showAuthor' | 'showDate' | 'showSha';

export interface ColumnToggle {
  key: ColumnKey;
  icon: typeof GitBranch;
  label: string;
}

export const COLUMN_TOGGLES: ColumnToggle[] = [
  { key: 'refsColumn', icon: GitBranch, label: 'Branch / Tag' },
  { key: 'showAuthor', icon: User, label: 'Author' },
  { key: 'showDate', icon: Clock, label: 'Date' },
  { key: 'showSha', icon: Hash, label: 'SHA' },
];
