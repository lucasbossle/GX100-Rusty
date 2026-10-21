#!/usr/bin/env node
// Extracts the GX-100 parameter tables from an installed copy of "BOSS TONE STUDIO for GX-100" into
// ./extracted (gitignored: derived from Roland's files, never publish it). The editor does the same by
// itself at first launch (gx100-extract crate); this Node version is for development: it also writes the
// intermediate JSON files, for inspection and for manual.mjs.
//
// usage: node tools/extract/extract.mjs [path/to/BOSS TONE STUDIO for GX-100.app] [outDir]

import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const DEFAULT_APP = '/Applications/BOSS/GX-100/BOSS TONE STUDIO for GX-100.app';
const appPath = process.argv[2] || DEFAULT_APP;
const outDir = process.argv[3] || path.resolve('extracted');
const html = path.join(appPath, 'Contents/Resources/html');

// core.js defines extractTables() and tablesBundle(); run it here, the official editor's scripts in a sandbox.
vm.runInThisContext(fs.readFileSync(new URL('./core.js', import.meta.url), 'utf8'), { filename: 'core.js' });
const ctx = vm.createContext({ console, Object, Math, JSON });
const t = globalThis.extractTables({
  read: (rel) => fs.readFileSync(path.join(html, rel), 'utf8'),
  run: (src, name) => vm.runInContext(src, ctx, { filename: name }),
  global: ctx,
  log: (msg) => console.log(msg),
});

fs.mkdirSync(outDir, { recursive: true });
const out = (f, d) => { fs.writeFileSync(path.join(outDir, f), JSON.stringify(d, null, 2) + '\n'); console.log(`wrote ${f}`); };
out('product.json', t.product);
out('address_map.blocks.json', t.blocks);
out('address_map.flat.json', t.flat);
out('commands.json', t.commands);
out('effects.json', t.effects);
out('assign.json', t.assign);
out('resources.json', t.resources);
out('param_values.json', t.paramValues);
out('tables.json', globalThis.tablesBundle(t));
