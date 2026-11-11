# Git Viewer

A fast, minimal git repository viewer for macOS — in the spirit of GitKraken. Read-only apart from a
small set of explicit actions: stage / unstage and commit, branch and tag creation, checkout, cherry-pick,
revert and reset from a commit's menu, stash push / pop, and fetch / pull / push through your own git
setup. Tauri 2 (Rust, shells out to the `git` CLI) + Svelte 5 (HTML/CSS UI).

## Install

Requires macOS 13.3+ and git (Homebrew or Xcode CLT; override with `GIT_VIEWER_GIT`).

Prebuilt downloads are not currently available. To build the app locally, follow
[Development](#development).

## Features

- Commit graph with stable lanes (HEAD pinned to the left), ref badges, WIP row, stash rows
- Commit details, filterable file lists (list/tree), unified/split diffs with syntax + intra-line
  highlighting, image diffs, streamed huge diffs that never buffer the whole patch
- Working tree status (staged / unstaged / untracked / conflicted) for every worktree
- Branches (lazy ahead/behind), remotes, tags, stashes, worktrees, submodules — hide / solo refs
- Compare two commits, a commit against the working tree, or a ⇧-selected range
- Blame (with -w), file history (follows renames), file tree browser at any revision; viewers stack
  (diff → blame → back to the diff)
- Graph search (subject/author/sha/refs instantly; message/path/content via git, cancellable)
- Tabs and named groups of tabs (drag a tab onto a group; the open group unfolds in the bar, its
  tabs on a tinted tray), live refresh via FSEvents (gitignored churn like
  `node_modules/` or `target/` is ignored), keyboard driven, light / dark / system themes, zoom
- Remote branches stay current: the active tab's remotes are fetched in the background every 5
  minutes, without ever prompting (turn it off under Settings ▸ fetch)

Nothing but those actions and that fetch writes to your repositories: every other git call runs with
optional locks disabled and working-tree diffs use plumbing commands that never touch the index.

## Usage

Open repositories with ⌘O or an empty tab's folder buttons (a Finder panel; ⌘-click picks several),
by dropping folders on the window, or by passing paths on the command line
(`"Git Viewer.app/Contents/MacOS/git-viewer" ~/code/repo`).

## Keyboard

| Keys | Action |
|---|---|
| ⌘O / ⌘T / ⌘W | open repository / new tab / close tab |
| ⇧⌘N | new tab group |
| ⌘1…⌘9, ⌃Tab, ⌥⌘← / ⌥⌘→ | switch tabs (a group's tabs count in the group's place) |
| ⌘R | refresh |
| ⌘F, ⌘G / ⌘⇧G | find in graph, next / previous match |
| ⌘K | toggle details |
| ⌘= / ⌘- / ⌘0 | zoom |
| ↑ ↓ (j k), PgUp PgDn, Home End | move selection |
| ⌥↓ | first parent |
| → / Enter | open the first changed file |
| ⌘C | copy the selected commit's sha |
| ⌘-click | compare with the selection (commit or WIP row) |
| ⇧-click, ⇧↑ ⇧↓ | select a commit range |
| ↑ ↓ ([ ]) in a diff | previous / next file |
| ← in a diff | back to the history |
| n / p | next / previous hunk |
| s / w | split view / ignore whitespace |
| Esc | back (previous viewer, then the graph) |

## Customizing the look

Settings → theme: choose light, dark, or system (the default). The choice is saved on this Mac;
system follows macOS appearance changes. Open tabs and viewers keep their position when switching.

All colors, sizes and fonts are CSS variables in [`src/styles/tokens.css`](src/styles/tokens.css);
shared utility classes live in [`src/styles/base.css`](src/styles/base.css). Components are Svelte
single-file components with scoped CSS that only reference those tokens.

Without rebuilding: settings popover → palette icon creates and reveals
`~/Library/Application Support/dev.local.gitviewer/user.css`, which is loaded after the built-in
theme (re-read whenever the window regains focus). Tints are `color-mix()` of their base token, so
an override recolors its family (e.g. `--accent` also moves the toggle fill and the focus ring). Keep the
surface, row-state and lane colors opaque (listed at the top of `tokens.css`). Dark bands and row
states mix `--surface-deep`, `--surface-press` and `--bg-1`; light row states mix ink over paper.
Adjust those surface tokens together when changing the palette. Example:

```css
:root {
  --radius: 4px;
}
:root[data-theme='light'] {
  --lane-0: #6d28d9;
}
:root[data-theme='dark'] {
  --lane-0: #c4b5fd;
}
```

## Development

Requires Node 20.19+ (or 22.12+) and Rust 1.90+.

```sh
npm install
npm run tauri dev                     # development
npm run tauri -- dev -- -- /path/repo # open a repo on launch
npm run tauri build                   # → src-tauri/target/release/bundle/macos/Git Viewer.app
```

- Checks: `cd src-tauri && cargo check && cargo test --lib`, `npm run check`, `npm run build`.
