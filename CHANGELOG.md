# Changelog

## 0.4.1

- **Feather is the default theme again.** Version 0.4.0 switched everyone to a new theme by mistake. This update puts you back on Feather.
- The new theme is now called **Claude** and is one of the choices in Settings, next to Feather.

## 0.4.0

- **A new default look**: Clay, a warm paper-and-terracotta theme from tweakcn, is what Feather now starts with, in light and dark. If you were on the Feather theme you are moved to Clay once; Feather is still there in Settings.
- **README images**: far more of them show up. GIF and WebP files (a GIF shows its first frame), HTML `<img>` tags at the size they ask for, paths from the top of the repository, addresses ending in `?raw=true`, and links to a picture's page on GitHub all work now. Pictures that live in the same repository on GitHub are read from your disk, so they also work in private repositories.
- An image that really cannot be loaded shows a small "Image unavailable" note instead of a red warning icon.
- **HTML in Markdown**: centered logos and headings, links, line breaks, `<details>`, `<kbd>` and other common tags are shown as what they mean rather than printed as tags.
- **Blame beside the line**: who last changed the line your cursor is on, when, and the commit summary, written faintly after the end of that line as well as in the status bar. Turn it off under Settings > Git > Show blame.
- **macOS**: install with one line in Terminal, `curl -fsSL https://feather.lh3k.dev/install.sh | bash`. It checks the download against the published checksum and puts Feather in Applications. A copy fetched this way is not held back the way a browser download is. This is not notarization, and the rest of macOS's protections still apply.

## 0.3.1

- **Terminal on Windows**: the terminal stayed blank and ignored typing. Windows asks a new terminal where its cursor is and waits for the answer; Feather now replies, so the prompt appears and input works.
- The search box grows as you type a longer query, and text now sits in the middle of every single-line field.
- Theme tiles in Settings no longer have a strip down their left edge.

## 0.3.0

- **Terminal**: open terminals inside the window with Ctrl+` (or the button in the title bar). They start in the repository folder, and you can keep several in tabs. The shell can be changed in Settings.
- **Search**: a new Search tab finds text across the repository, with options for case, whole words and regular expressions; click a result to open the file on that line. Ctrl+F finds within the open file, and Ctrl+P opens a file by typing part of its name.
- **Themes**: seven built-in themes (Feather, Midnight, Catppuccin, Gruvbox, Nord, Solarized, Tokyo Night), each with a light and a dark side, restyle the whole app. You can also import a theme by pasting the CSS from tweakcn.com or any shadcn/ui theme.
- **Blame**: the status bar under a file shows who last changed the current line, when, and the commit summary, next to the cursor position.
- **Discard all**: a button beside "Stage all" throws away every unstaged change, after asking.
- The sidebar tabs are now Files, Search and Git (Ctrl+1, 2 and 3).

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
