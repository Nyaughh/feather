# Changelog

## 0.2.1

- **Windows installer**: `feather-windows-x86_64-setup.exe` installs Feather for your user account with no administrator prompt, adds Start menu and desktop shortcuts and an uninstaller, and can add "Open with Feather" to the right-click menu for folders.
- **README badges**: badge images (shields.io and similar) now show up, with their text, instead of red warning icons.
- The plain `.exe` is still there for anyone who prefers to keep it in a folder of their own; both update themselves the same way.

Windows still shows its "protected your PC" box the first time, for the installer as for the plain program, because Feather is not code-signed yet. Choose **More info**, then **Run anyway**.

## 0.2.0

- **Edit files**: click into any text file and type. Save with Ctrl+S (Cmd+S on macOS), or turn on auto save. Feather asks before dropping unsaved changes.
- **Its own title bar**: the window buttons and a sidebar toggle now sit in a bar that matches the rest of the app.
- **Settings**: dark, light or system theme, accent colour, interface size, code font size, line wrap, line numbers, indentation and auto save. Open them from the gear in the title bar.
- **Updates**: Feather checks for new versions and offers to install them from a prompt in the corner. You can also check from Settings.
- **Branches from the remote**: the branch picker now lists branches that only exist on the remote, and checks them out with one click. Feather fetches every few minutes so new ones appear on their own (this can be turned off in Settings).
- Copy and paste now work everywhere, including the branch filter and the commit box, and links in Markdown open in your browser.
- The repository switcher shows just the name, and the worktree and branch pickers share one row.

This is the last version you need to download by hand: from here on Feather can update itself.

## 0.1.0

The first release of Feather.

- **Files**: a lazy file tree with git status markers, syntax-highlighted source, rendered Markdown, and image and SVG previews.
- **Git**: staged and unstaged changes with per-file diffs, plus stage, unstage, discard, commit, pull, push and fetch.
- **Branches and worktrees**: switch either from the bottom-left corner, or type a new name to create a branch.
- Changes made outside Feather show up within a couple of seconds, and an idle window does no drawing.

Feather needs `git` installed. The macOS and Windows builds are not signed yet, so each asks for confirmation the first time it is opened.
