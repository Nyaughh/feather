// Builds the website into dist/ for Vercel. The site is plain files, so this
// is only a copy.
//   node tools/build-site.mjs

import { cp, rm } from 'node:fs/promises';

await rm('dist', { recursive: true, force: true });
await cp('site', 'dist', { recursive: true });
console.log('Built dist/');
