#!/bin/bash
# Installs Feather on a Mac:
#
#   curl -fsSL https://feather.lh3k.dev/install.sh | bash
#
# It downloads the newest release from GitHub, checks it against the published
# SHA-256 checksum, and puts Feather.app in /Applications (or ~/Applications if
# that is not writable). It needs no sudo and changes no security setting.
#
# Why this route: Feather is not notarized by Apple. A copy downloaded in a
# browser is marked as such by the browser, and macOS then refuses to open it
# until you allow it in System Settings. A copy fetched by curl carries no such
# mark. This is not notarization, and the usual macOS protections still apply.
set -euo pipefail

REPO='Nyaughh/feather'
APP='Feather.app'
APP_ID='dev.lh3k.feather'
ASSET='feather-macos-universal.zip'
BASE="https://github.com/$REPO/releases/latest/download"

tmp=''
stage=''
dest=''

log()  { printf '[feather] %s\n' "$*"; }
fail() { printf '[feather] Error: %s\n' "$*" >&2; exit 1; }

cleanup() {
  [[ -n "$tmp" ]] && rm -rf "$tmp"
  if [[ -n "$stage" ]]; then
    # Interrupted between the two moves: put the old copy back.
    if [[ -n "$dest" && -e "$stage/old.app" && ! -e "$dest" ]]; then
      mv "$stage/old.app" "$dest"
    fi
    rm -rf "$stage"
  fi
  return 0
}

fetch() {
  curl -fsSL --proto '=https' --tlsv1.2 --retry 3 "$BASE/$1" -o "$tmp/$1" \
    || fail "Could not download $1."
}

app_id() {
  /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist" 2>/dev/null || true
}

main() {
  [[ "$(uname -s)" == Darwin ]] || fail 'This installer is for macOS.'
  command -v git >/dev/null 2>&1 \
    || log 'Note: Feather needs git. To get it, run: xcode-select --install'

  tmp="$(mktemp -d "${TMPDIR:-/tmp}/feather.XXXXXX")"
  trap cleanup EXIT

  log 'Downloading the latest release...'
  fetch "$ASSET"
  fetch SHA256SUMS.txt

  local expected actual
  expected="$(awk -v f="$ASSET" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS.txt")"
  [[ -n "$expected" ]] || fail 'No checksum published for the macOS download.'
  actual="$(shasum -a 256 "$tmp/$ASSET" | awk '{ print $1 }')"
  [[ "$actual" == "$expected" ]] || fail 'Checksum mismatch. Nothing was installed.'

  ditto -x -k "$tmp/$ASSET" "$tmp/unpacked"
  local app="$tmp/unpacked/$APP"
  [[ -d "$app" && ! -L "$app" ]] || fail 'The download has no Feather.app.'
  [[ "$(app_id "$app")" == "$APP_ID" ]] || fail 'Unexpected app identity.'
  codesign --verify --deep --strict "$app" || fail 'App signature check failed.'

  local dir
  if [[ -e "/Applications/$APP" || -w /Applications ]]; then
    dir=/Applications
  else
    dir="$HOME/Applications"
  fi
  mkdir -p "$dir"
  [[ -w "$dir" ]] || fail "Cannot write to $dir."

  dest="$dir/$APP"
  if [[ -e "$dest" ]]; then
    [[ "$(app_id "$dest")" == "$APP_ID" ]] \
      || fail "Refusing to replace a different app at $dest."
    if pgrep -x feather >/dev/null 2>&1; then
      fail 'Quit Feather, then run this again.'
    fi
  fi

  # Stage on the same disk, then swap, so a failed install leaves the old copy in place.
  stage="$(mktemp -d "$dir/.feather-install.XXXXXX")"
  ditto "$app" "$stage/$APP"
  if [[ -e "$dest" ]]; then
    mv "$dest" "$stage/old.app"
  fi
  mv "$stage/$APP" "$dest" || fail 'Install failed.'

  log "Installed Feather at $dest"
  open "$dest" || log 'Open it from Applications.'
}

# Everything runs from here, so a download cut off halfway runs nothing.
main
