# Changelog

## 0.6.2

- **A new look**: Feather has a real logo now, a feather drawn by a designer, on a warm "golden-hour" tile that fades from cream to a soft rose-peach. It is the app icon on every system and is used across the website.
- **Golden-hour blush is the default accent**: buttons, selections, links and focus rings take the blush in dark mode and a deeper terracotta in light mode, so text on them stays easy to read. Choose a different accent under **Settings > Accent colour**: Monochrome, Deep blue, Teal, Soft violet, Indigo, Orange or Pink, or keep the theme's own. Accents you had picked before are kept.
- **Terminal on the right**: the terminal's "..." menu has **Dock at Bottom** and **Dock on Right**, so it can sit beside your files as a tall panel. Feather remembers where you put it and how big it was in each place.
- **Split terminals**: split a terminal tab to run shells side by side (stacked when the terminal is on the right) with the split button, the "..." menu, or Ctrl+Shift+5 (Cmd+\ on macOS). Click a split or press Ctrl+Alt+Left/Right to move between them, and close one from its own title strip.
- **The `feather` command**: open things from a terminal the way `code` opens VS Code. `feather .` or `feather some-folder` opens a folder, and `feather main.rs` opens a file (a new file is created when you save it). If the folder is already open, that window comes to the front. Try `feather --help`. The Windows installer puts `feather` on your PATH; on macOS and Linux, use **Settings > Command line > Install**.
- **Several windows**: open more than one repository at once. Each window has its own folder, tabs, terminals and Git view, and closing one leaves the others open.
- **File menu**: at the top left of the window, with New Window (Ctrl+Shift+N), Open File (Ctrl+Shift+O), Open Folder (Ctrl+O), Open Recent, Close Folder and Close Window (Ctrl+Shift+W). Close Folder takes you back to the start screen with your recent folders.
- Settings and recent folders are shared between windows, and a change in one window shows up in the others.
- Ctrl+Shift+` opens a new terminal again, instead of hiding the panel.

## 0.6.1

- **Commits, more like VS Code**: the list now sits under Pull, Push and Fetch and starts folded; Feather remembers whether you opened it. Each row shows the author after the message, and branches and tags as labelled pills (your current branch highlighted, remote branches with a cloud, tags with a tag). Rest the pointer on a commit for a card beside the sidebar with the author, the exact date, the whole message, how many files and lines it changed, its branches, and buttons to copy the hash or open it on GitHub.
- **The "..." menu works like VS Code's**: Pull, Push and Fetch, then **Commit**, **Changes** and **Stash** open their own menus to the side. Commit has Commit Staged, Commit All, Undo Last Commit and the two Amend options; Changes has stage, unstage and discard all; Stash has stash, pop and apply latest, and every stash with its own Pop, Apply and Delete.
- The copy button on code blocks in Markdown is centred on one-line blocks and level with the first line of longer ones.
- Text in the Go to file and branch search fields sits in the middle of the field, and the longer messages in empty panels are centred line by line.
- The commit view shows the exact date beside "committed 6 hours ago".

## 0.6.0

- **Commit history**: the Git tab now lists recent commits under your changes, with the branch graph drawn beside them: a line per branch, merges where lines join, and your current commit ringed. Commits on the remote that you have not pulled yet show hollow and dimmed, and the heading counts them. Click a commit to see its message and every file it changed. The list can be folded away or resized by dragging its top edge.
- **Stashes moved into the "..." menu**: choose **Stashes** there to see them, then pick one to apply, apply and keep, or delete. They no longer fill the Git tab.
- The "..." button now has the same shape as the Commit button beside it.

## 0.5.0

- **Stash**: the new "..." button beside Commit has **Stash changes**, which sets every change aside, new files included. Stashes are listed under the changes; click one to apply and remove it, apply and keep it, or delete it.
- **Amend last commit**: adds what is staged to the last commit, and uses the message in the box if you wrote one.
- **Undo last commit**: takes the last commit back, leaves its changes staged and puts its message back in the box. Amend and undo are only offered while the commit has not been pushed.
- **Worktrees inside the repository** (a `.worktrees` folder, for example) no longer show up as untracked changes. They are checkouts of the same repository, which is also why discarding them did nothing.
- Discarding a folder that is a separate git repository now says that Feather will not delete it, instead of silently doing nothing.
- Stashing or applying a stash while a file has unsaved edits asks about them first.

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
