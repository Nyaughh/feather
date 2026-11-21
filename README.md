# Feather

A fast, minimal desktop app for browsing a repository and handling everyday git.

**[feather.lh3k.dev](https://feather.lh3k.dev)** · **[Download](https://github.com/Nyaughh/feather/releases/latest)** for Windows, macOS and Linux

![Feather showing a diff in its Git tab](site/shots/diff.webp)

- **Files**: a lazy file tree with git status markers; a syntax-highlighted
  editor, rendered Markdown, and image/SVG previews.
- **Git**: staged and unstaged changes, per-file diffs, stage / unstage / discard,
  commit, amend and undo the last commit, stash, and pull / push / fetch. Recent
  history with a branch graph; click a commit to see what it changed.
- **Branches and worktrees**: side by side at the bottom-left; click either to
  switch, or type a new name in the branch picker to create one. Branches that
  only exist on the remote are listed too, and Feather fetches every few minutes.
- **Search**: text across the repository, find in the open file, and quick open
  by file name.
- **Terminal**: tabbed terminals in the repository folder, inside the window.
  Split a tab to run shells side by side, and dock the panel at the bottom or
  on the right from its `...` menu.
- **Themes**: eight built in, each light and dark, and any tweakcn or shadcn/ui
  theme can be pasted in. Plus accent colour, font size, line wrap and indentation.
- **Blame**: who last changed the current line, beside the line and in the status
  bar. It can be turned off in Settings.
- **Updates**: Feather checks for new releases and can install them itself.

Feather is written in Rust with [egui](https://github.com/emilk/egui). It ships as a
single native binary, only repaints when something changes, and does all disk and
git work off the UI thread.

## Install

**macOS** - paste this into Terminal:

```sh
curl -fsSL https://feather.lh3k.dev/install.sh | bash
```

It downloads the newest release, checks it against the published checksum and
puts Feather in Applications, with no password and no settings changed. Feather
isn't notarized by Apple, and macOS refuses to open such an app when a browser
downloaded it; a copy fetched this way isn't held back. This is not
notarization, and the rest of macOS's protections still apply. You can read
[the script](site/install.sh) first.

Or download `feather-macos-universal.zip` from the
[latest release](https://github.com/Nyaughh/feather/releases/latest) and drag
Feather into Applications. If macOS blocks it, open System Settings > Privacy &
Security > Open Anyway.

**Windows** - run `feather-windows-x86_64-setup.exe` from the
[latest release](https://github.com/Nyaughh/feather/releases/latest).

**Linux** - unpack `feather-linux-x86_64.tar.gz` from the
[latest release](https://github.com/Nyaughh/feather/releases/latest) and run
`./feather`.

The [website](https://feather.lh3k.dev/#install) has the steps for each system.
Feather needs `git` on your `PATH`, and keeps itself up to date after that.

## The `feather` command

Open files and folders from a terminal, the way `code` works with VS Code:

```sh
feather .              # this folder as the workspace
feather t2chat         # another folder
feather src/main.rs    # a file, in the repository it belongs to
feather notes.md       # a file that doesn't exist yet: opens empty, created on save
feather                # just start Feather (it reopens the last repository)
```

Paths are relative to the terminal's current folder, and the command returns
to the prompt straight away. If a Feather window already shows that folder (or
the repository a file is in), that window comes to the front and opens it; a
window with nothing open is reused too. Otherwise a new window opens.

| Option               | Effect                                              |
|----------------------|-----------------------------------------------------|
| `-n`, `--new-window`   | Always open a new window                            |
| `-r`, `--reuse-window` | Open in the most recent window, replacing its folder |
| `-w`, `--wait`         | Open a new window and return when it closes         |
| `-h`, `--help`         | Show the options                                    |
| `-v`, `--version`      | Show the version                                    |

Putting the command on your `PATH`:

- **Windows**: the installer does it (the *Add the "feather" command to PATH*
  option, on by default). Open a new terminal afterwards. It works in
  PowerShell, Command Prompt and Git Bash, and the uninstaller removes it.
  The command is `bin\feather.exe`, a small console program (built from
  `src/bin/feather-launcher.rs`) that runs Feather with the same arguments.
- **macOS**: in Feather, open Settings > Command line > **Install**. This links
  `/usr/local/bin/feather` to the app, asking for your password if that folder
  needs it.
- **Linux**: Settings > Command line > **Install** links `~/.local/bin/feather`
  to the program. Most distributions already have `~/.local/bin` on `PATH`;
  Feather says so if yours doesn't.

## Building from source

You need a [Rust toolchain](https://rustup.rs).

```sh
cargo run --release            # reopens the last repository
cargo run --release -- ~/code/my-repo
```

The optimized binary is written to `target/release/feather`. A build only hands
paths to windows of the same program file, so one built from source and an
installed Feather keep to their own windows. Debug builds stay attached to the
terminal instead of returning to the prompt, so their output stays visible.

## Shortcuts

`Ctrl` is `Cmd` on macOS.

| Shortcut         | Action                         |
|------------------|--------------------------------|
| `Ctrl+O`         | Open a repository              |
| `Ctrl+P`         | Go to file                     |
| `Ctrl+Shift+F`   | Search in files                |
| `Ctrl+F`         | Find in the open file          |
| `Ctrl+S`         | Save the file being edited     |
| `` Ctrl+` ``     | Show or hide the terminal      |
| `` Ctrl+Shift+` `` | New terminal                 |
| `Ctrl+Shift+5`   | Split the terminal (`Cmd+\` on macOS) |
| `Ctrl+Alt+Left` `Right` | Previous or next split  |
| `Ctrl+1` `2` `3` | Files, Search and Git tabs     |
| `Ctrl+Enter`     | Commit (in the Git tab)        |
| `Ctrl+B`         | Hide or show the sidebar       |
| `Ctrl+,`         | Settings                       |
| `Ctrl+R`         | Refresh from disk              |

## How it works

| Module        | Role                                                                 |
|---------------|----------------------------------------------------------------------|
| `git.rs`      | Runs the `git` CLI: status, diff, branches, search (grep), blame.    |
| `worker.rs`   | Background threads; results come back to the UI as events.           |
| `document.rs` | Loads files and syntax-highlights them.                              |
| `diff.rs`     | Parses unified diffs into rows.                                      |
| `graph.rs`    | Lays out the lines of the commit graph.                              |
| `tree.rs`     | The lazily-read file tree.                                           |
| `app.rs`      | Application state; applies UI commands and worker events.            |
| `ui/`         | Drawing: title bar, sidebar, editor and diffs, settings, widgets.    |
| `theme.rs`    | The current colors, fonts and metrics.                               |
| `themes.rs`   | The built-in themes, and importing one from tweakcn / shadcn CSS.    |
| `terminal.rs` | Shells running in pseudo-terminals, and their screen state.          |
| `images.rs`   | Finds and decodes every image shown, including README badges.        |
| `markdown.rs` | Turns the HTML in a README into Markdown the viewer can show.        |
| `settings.rs` | Everything the user can change, saved between runs.                  |
| `update.rs`   | Checks GitHub for a newer release and installs it.                   |

Feather shells out to `git` rather than linking a git library, so commits use your
hooks, signing and credential helpers exactly as they do in a terminal.

A background thread re-checks git status and the open file or diff every two
seconds while the window is focused (and immediately when it regains focus). The
window is only redrawn when one of them actually changed.

## Releasing

1. Set the new version in `Cargo.toml` and add a `## <version>` section to
   `CHANGELOG.md`; that section becomes the release notes.
2. Run the **Release** workflow from the Actions tab with that version. It
   builds the three downloads, then creates the tag and the GitHub release.

The website (`site/`, deployed by Vercel) reads releases from GitHub when it
loads, so it picks up a new one without a redeploy. Installed copies find the
release the same way and offer to update, which relies on the download names
(`feather-windows-x86_64.exe` and so on) and `SHA256SUMS.txt` staying the same.

## Tests

```sh
cargo test
```

## Fonts

The bundled [Inter](https://rsms.me/inter/) (SIL Open Font License) and
[Phosphor Icons](https://phosphoricons.com) (MIT) fonts are in `assets/fonts`
alongside their licenses.
