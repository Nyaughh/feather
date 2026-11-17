// feather.lh3k.dev: picks the right download for the visitor's computer, and
// fills the release section from GitHub on each visit so a new release shows
// up here without a redeploy.

const REPO = 'Nyaughh/feather';

/** The three downloads. File names are the same in every release. */
const PLATFORMS = {
  mac: { name: 'macOS', asset: 'feather-macos-universal.zip', card: 'os-mac' },
  // Releases before 0.2.1 have no installer, only the program on its own.
  windows: { name: 'Windows', asset: 'feather-windows-x86_64-setup.exe', older: 'feather-windows-x86_64.exe', card: 'os-windows' },
  linux: { name: 'Linux', asset: 'feather-linux-x86_64.tar.gz', card: 'os-linux' },
};

const latestUrl = (asset) => `https://github.com/${REPO}/releases/latest/download/${asset}`;

// ── Which computer is this? ──────────────────────────────────────────────────

function detectPlatform() {
  const hint = `${navigator.userAgentData?.platform ?? ''} ${navigator.platform ?? ''} ${navigator.userAgent}`.toLowerCase();
  // Phones and tablets can't run Feather; they get the plain "Download" button.
  if (/android|iphone|ipad|ipod/.test(hint) || (/mac/.test(hint) && navigator.maxTouchPoints > 1)) return null;
  if (/mac/.test(hint)) return 'mac';
  if (/win/.test(hint)) return 'windows';
  if (/linux|x11|cros/.test(hint)) return 'linux';
  return null;
}

const platform = detectPlatform();
const heroButton = document.getElementById('heroDownload');
const heroMeta = document.getElementById('heroMeta');

if (platform) {
  const { name, asset, card } = PLATFORMS[platform];
  if (platform === 'mac') {
    // On a Mac the one-line installer is the way in: a zip downloaded in a
    // browser is blocked by macOS until it is allowed in System Settings.
    heroButton.href = `#${card}`;
    document.getElementById('heroDownloadLabel').textContent = 'Install on macOS';
  } else {
    heroButton.href = latestUrl(asset);
    heroButton.dataset.asset = asset;
    document.getElementById('heroDownloadLabel').textContent = `Download for ${name}`;
  }
  document.getElementById(card).classList.add('yours');
  // In the release card, only this computer's download stays highlighted.
  for (const link of document.querySelectorAll('.release-side .btn')) {
    link.classList.toggle('primary', link.dataset.asset === asset);
  }
}
if (platform === 'mac') {
  for (const key of document.querySelectorAll('kbd[data-mod]')) key.textContent = '⌘';
}

// ── Copy buttons ─────────────────────────────────────────────────────────────

for (const button of document.querySelectorAll('button[data-copy]')) {
  const label = button.textContent;
  button.addEventListener('click', async () => {
    const source = document.getElementById(button.dataset.copy);
    try {
      await navigator.clipboard.writeText(source.textContent.trim());
      button.textContent = 'Copied';
    } catch {
      // No clipboard access: select the text so Cmd+C does it.
      getSelection().selectAllChildren(source);
      button.textContent = 'Selected, now copy it';
    }
    setTimeout(() => { button.textContent = label; }, 1800);
  });
}

// ── Releases ─────────────────────────────────────────────────────────────────

const CACHE_KEY = 'feather.releases';
const CACHE_MS = 10 * 60 * 1000; // GitHub allows 60 anonymous requests an hour

async function fetchReleases() {
  try {
    const cached = JSON.parse(sessionStorage.getItem(CACHE_KEY));
    if (cached && Date.now() - cached.at < CACHE_MS) return cached.list;
  } catch { /* no storage, or nothing stored */ }

  const response = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=6`, {
    headers: { Accept: 'application/vnd.github+json' },
  });
  if (!response.ok) throw new Error(`GitHub answered ${response.status}`);
  const list = (await response.json())
    .filter((release) => !release.draft && !release.prerelease)
    .map((release) => ({
      tag: release.tag_name,
      name: release.name || release.tag_name,
      date: release.published_at,
      notes: release.body || '',
      page: release.html_url,
      assets: Object.fromEntries(release.assets.map((asset) => [asset.name, { url: asset.browser_download_url, size: asset.size }])),
    }));
  try { sessionStorage.setItem(CACHE_KEY, JSON.stringify({ at: Date.now(), list })); } catch { /* private mode */ }
  return list;
}

const day = (iso) => new Date(iso).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
const megabytes = (bytes) => `${(bytes / 1048576).toFixed(1)} MB`;

function el(tag, props = {}, ...children) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

/** **bold**, `code` and [links](https://…) inside one line of release notes. */
function inline(tag, text) {
  const node = document.createElement(tag);
  for (const part of text.split(/(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\(https?:\/\/[^)\s]+\))/)) {
    const link = part.match(/^\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)$/);
    if (link) node.append(el('a', { href: link[2] }, link[1]));
    else if (part.startsWith('**')) node.append(el('b', {}, part.slice(2, -2)));
    else if (part.startsWith('`')) node.append(el('code', {}, part.slice(1, -1)));
    else if (part) node.append(part);
  }
  return node;
}

/** Release notes are Markdown; paragraphs and bullet lists are all they use. */
function renderNotes(markdown) {
  const out = document.createDocumentFragment();
  let list = null;
  for (const raw of markdown.split(/\r?\n/)) {
    const line = raw.trim();
    const item = line.match(/^[-*]\s+(.*)/);
    if (item) {
      if (!list) out.append(list = document.createElement('ul'));
      list.append(inline('li', item[1]));
    } else {
      list = null;
      if (line) out.append(inline('p', line.replace(/^#+\s*/, '')));
    }
  }
  return out;
}

function showReleases([latest, ...older]) {
  document.getElementById('relName').textContent = latest.name;
  document.getElementById('relMeta').textContent = `${latest.tag} · ${day(latest.date)}`;
  document.getElementById('relNotes').replaceChildren(renderNotes(latest.notes));

  // Point every download at this exact release, and show how big it is.
  for (const link of document.querySelectorAll('[data-asset]')) {
    const asset = latest.assets[link.dataset.asset];
    if (!asset) continue;
    link.href = asset.url;
    const size = link.querySelector('.size');
    if (size) size.textContent = megabytes(asset.size);
  }
  const mine = platform && latest.assets[PLATFORMS[platform].asset];
  heroMeta.textContent = [latest.tag, mine && megabytes(mine.size), 'Needs git installed'].filter(Boolean).join(' · ');

  document.getElementById('older').replaceChildren(...older.map((release) => el('li', {},
    el('span', { className: 'name' }, release.name),
    el('span', { className: 'meta' }, `${release.tag} · ${day(release.date)}`),
    ...Object.values(PLATFORMS)
      .map(({ name, asset, older }) => ({ name, file: release.assets[asset] ?? release.assets[older] }))
      .filter(({ file }) => file)
      .map(({ name, file }) => el('a', { href: file.url }, name)),
    el('a', { href: release.page }, 'Notes'),
  )));
}

fetchReleases()
  .then((list) => {
    if (list.length) showReleases(list);
    else document.getElementById('relMeta').textContent = 'No release yet.';
  })
  .catch(() => {
    // The download buttons already point at GitHub's "latest" address.
    document.getElementById('relMeta').textContent = 'Could not reach GitHub for the version details. The download buttons still get the newest one.';
  });
