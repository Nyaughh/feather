# Feather

A fast, minimal desktop app for browsing a repository and handling everyday git.

- **Files** — a lazy file tree with git status markers; syntax-highlighted source,
  rendered Markdown, and image/SVG previews.
- **Git** — staged and unstaged changes, per-file diffs, stage / unstage / discard,
  commit, and pull / push / fetch.
- **Branches and worktrees** — pinned to the bottom-left; click either to switch,
  or type a new name in the branch picker to create one.

Feather is written in Rust with [egui](https://github.com/emilk/egui). It ships as a
single native binary, only repaints when something changes, and does all disk and
git work off the UI thread.

## Running

You need a [Rust toolchain](https://rustup.rs) and `git` on your `PATH`.

```sh
cargo run --release            # reopens the last repository
cargo run --release -- ~/code/my-repo
```

The optimized binary is written to `target/release/feather`.

## Shortcuts

`Ctrl` is `Cmd` on macOS.

| Shortcut       | Action                    |
|----------------|---------------------------|
| `Ctrl+O`       | Open a repository         |
| `Ctrl+1` / `2` | Files tab / Git tab       |
| `Ctrl+Enter`   | Commit (in the Git tab)   |
| `Ctrl+R`       | Refresh from disk         |

## How it works

| Module        | Role                                                                 |
|---------------|----------------------------------------------------------------------|
| `git.rs`      | Runs the `git` CLI and parses its output (status, diff, branches…).  |
| `worker.rs`   | Background threads; results come back to the UI as events.           |
| `document.rs` | Loads files and syntax-highlights them.                              |
| `diff.rs`     | Parses unified diffs into rows.                                      |
| `tree.rs`     | The lazily-read file tree.                                           |
| `app.rs`      | Application state; applies UI commands and worker events.            |
| `ui/`         | Drawing: sidebar, content area, shared widgets.                      |
| `theme.rs`    | Colors, fonts and metrics.                                           |

Feather shells out to `git` rather than linking a git library, so commits use your
hooks, signing and credential helpers exactly as they do in a terminal.

A background thread re-checks git status and the open file or diff every two
seconds while the window is focused (and immediately when it regains focus). The
window is only redrawn when one of them actually changed.

## Tests

```sh
cargo test
```

## Fonts

The bundled [Inter](https://rsms.me/inter/) (SIL Open Font License) and
[Phosphor Icons](https://phosphoricons.com) (MIT) fonts are in `assets/fonts`
alongside their licenses.
