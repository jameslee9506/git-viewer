# Git Viewer

A fast, minimal, **read-only** git repository viewer for macOS — in the spirit of GitKraken, without
any write operations. Tauri 2 (Rust, shells out to the `git` CLI) + Svelte 5 (HTML/CSS UI).

- Commit graph with stable lanes (HEAD pinned to the left), ref badges, WIP row, stash rows
- Commit details, filterable file lists (list/tree), unified/split diffs with syntax + intra-line
  highlighting, image diffs, streamed huge diffs that never buffer the whole patch
- Working tree status (staged / unstaged / untracked / conflicted) for every worktree
- Branches (lazy ahead/behind), remotes, tags, stashes, worktrees, submodules — hide / solo refs
- Compare two commits, a commit against the working tree, or a ⇧-selected range
- Blame (with -w), file history (follows renames), file tree browser at any revision; viewers stack
  (diff → blame → back to the diff)
- Graph search (subject/author/sha/refs instantly; message/path/content via git, cancellable)
- Tabs, live refresh via FSEvents (gitignored churn like `node_modules/` or `target/` is ignored),
  keyboard driven, monotone dark-navy theme, zoom

Nothing in the app writes to your repositories: every git call runs with optional locks disabled and
working-tree diffs use plumbing commands that never touch the index.

## Requirements

- macOS 13.3+ (the UI is built for Safari 16.4, the system WebKit that 13.3 ships)
- Node 20.19+ (or 22.12+), Rust 1.90+
- git (Homebrew `/opt/homebrew/bin/git` preferred, Xcode CLT git works; override with `GIT_VIEWER_GIT`)

## Run

```sh
npm install
npm run tauri dev                      # development
npm run tauri -- dev -- -- /path/repo  # open a repo on launch
npm run tauri build                    # → src-tauri/target/release/bundle/macos/Git Viewer.app
```

Open repositories with ⌘O or an empty tab's folder buttons (a Finder panel; ⌘-click picks several),
by dropping folders on the window, or by passing paths on the command line
(`"Git Viewer.app/Contents/MacOS/git-viewer" ~/code/repo`).

## Keyboard

| Keys | Action |
|---|---|
| ⌘O / ⌘T / ⌘W | open repository / new tab / close tab |
| ⌘1…⌘9, ⌃Tab | switch tabs |
| ⌘R | refresh |
| ⌘F, ⌘G / ⌘⇧G | find in graph, next / previous match |
| ⌘J / ⌘K | toggle sidebar / details |
| ⌘= / ⌘- / ⌘0 | zoom |
| ↑ ↓ (j k), PgUp PgDn, Home End | move selection |
| ⌥↓ | first parent |
| → / Enter | open the first changed file |
| ⌘C | copy the selected commit's sha |
| ⌘-click | compare with the selection (commit or WIP row) |
| ⇧-click, ⇧↑ ⇧↓ | select a commit range |
| [ / ] | previous / next file in a diff |
| n / p | next / previous hunk |
| s / w | split view / ignore whitespace |
| Esc | back (previous viewer, then the graph) |

## Customizing the look

All colors, sizes and fonts are CSS variables in [`src/styles/tokens.css`](src/styles/tokens.css);
shared utility classes live in [`src/styles/base.css`](src/styles/base.css). Components are Svelte
single-file components with scoped CSS that only reference those tokens.

Without rebuilding: settings popover → palette icon creates and reveals
`~/Library/Application Support/dev.local.gitviewer/user.css`, which is loaded after the built-in
theme (re-read whenever the window regains focus). Example:

```css
:root {
  --bg-1: #0b1020;
  --accent: #8ab4f8;
  --lane-0: #8ab4f8;
}
```

## Development notes

- Checks: `cd src-tauri && cargo check && cargo test --lib`, `npm run check`, `npm run build`.
