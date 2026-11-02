// Copies the connector (web/) and the app icons into dist/mangavox/, which the Worker serves.
// The icons live with the app's other assets (the repository keeps images only there).
import { cpSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, 'dist', 'mangavox');
// Overwrite in place: a running `wrangler dev` watches dist/ and keeps it open.
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
cpSync(join(here, 'web'), out, { recursive: true });
const icons = join(here, '..', 'native', 'crates', 'mv-ui', 'assets', 'phone');
for (const f of ['icon.png', 'icon-512.png', 'logo.png']) cpSync(join(icons, f), join(out, f));
console.log('built', out);
