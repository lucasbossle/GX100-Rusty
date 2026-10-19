#!/usr/bin/env node
// Extracts the GX-100 SysEx protocol tables from an installed copy of
// "BOSS TONE STUDIO for GX-100" into JSON. The output is derived from
// Roland's files: it goes to ./extracted (gitignored) and must not be published.
//
// usage: node tools/extract/extract.mjs [path/to/BOSS TONE STUDIO for GX-100.app] [outDir]

import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const DEFAULT_APP = '/Applications/BOSS/GX-100/BOSS TONE STUDIO for GX-100.app';
const appPath = process.argv[2] || DEFAULT_APP;
const outDir = process.argv[3] || path.resolve('extracted');
const js = path.join(appPath, 'Contents/Resources/html/js');
const read = (rel) => fs.readFileSync(path.join(js, rel), 'utf8').replace(/^\uFEFF/, '');

// ---------------------------------------------------------------- sandbox
const ctx = vm.createContext({ console, Object, Math, JSON });
ctx.window = ctx;
const run = (src, name) => vm.runInContext(src, ctx, { filename: name });
run(read('utilities/constant.js'), 'constant.js');
run(read('utilities/converter.js'), 'converter.js');

// ------------------------------------------------------- product settings
const ps = read('config/product_setting.js');
const pick = (key) => { const m = ps.match(new RegExp(`\\b${key}:\\s*'?([^',\\s]+)'?`)); return m && m[1]; };
const product = {
  name: pick('name'), btsVersion: pick('version'), btsBuild: pick('build'),
  modelId: pick('modelId'), deviceId: pick('deviceId'),
  lengthOfAddr: +pick('lengthOfAddr'), lengthOfSize: +pick('lengthOfSize'),
  communicationLevel: +pick('communicationLevel'), familyCode: pick('familyCode'),
  rq1TimeoutSec: +pick('timeout'), extension: pick('extension'),
};
ctx.ProductSetting = { ...product, communicationLevel: product.communicationLevel };

// --------------------------------------------------------------- helpers
const hex7 = (lin) => { // linear -> "10 00 11 01"
  const p = ctx._7bitize(lin);
  return [24, 16, 8, 0].map((s) => ((p >>> s) & 0x7f).toString(16).padStart(2, '0').toUpperCase()).join(' ');
};
const hexPacked = (packed) => hex7(ctx.nibble(packed));
const ENC = {};
for (const k of ['INTEGER1x1','INTEGER1x2','INTEGER1x3','INTEGER1x4','INTEGER1x5','INTEGER1x6','INTEGER1x7','INTEGER2x4','INTEGER2x7','INTEGER4x4'])
  ENC[ctx[k]] = k;
const BYTES = { INTEGER2x4: 2, INTEGER2x7: 2, INTEGER4x4: 4 };
const encoding = (size) => {
  if (ENC[size]) return { encoding: ENC[size], bytes: BYTES[ENC[size]] || 1 };
  if (size & ctx.PADDING) return { encoding: 'PADDING', bytes: size & 0xffff };
  return { encoding: 'BYTES', bytes: size };
};

// ------------------------------------------------------------ address map
let amSrc = read('config/address_map.js');
const blockNames = [...amSrc.matchAll(/^var (\w+) = \[/gm)].map((m) => m[1]);
amSrc = amSrc.replace(/^this\.root = \[/m,
  `this.__blocks = {${blockNames.map((n) => `${n}: ${n}`).join(', ')}};\nthis.root = [`);
run(amSrc, 'address_map.js');
const am = vm.runInContext('new AddressMap()', ctx);
const span = (arr) => Math.max(0, ...arr.map((e) => e.addr + ('child' in e ? span(e.child) : encoding(e.size).bytes)));
const nameOf = new Map(Object.entries(am.__blocks).map(([k, v]) => [v, k]));

const blocks = {};
for (const [bname, arr] of Object.entries(am.__blocks)) {
  blocks[bname] = arr.map((e) => {
    if ('child' in e) return { offset: hex7(e.addr), name: e.name, block: nameOf.get(e.child), size: hex7(span(e.child)) };
    return {
      offset: hex7(e.addr), name: e.name, desc: e.desc, ...encoding(e.size),
      min: e.min, max: e.max, init: e.init, ofs: e.ofs, factor: e.factor,
    };
  });
}
const root = am.root.map((e) => ({ addr: hex7(e.addr), name: e.name, block: nameOf.get(e.child), size: hex7(span(e.child)) }));

// flat, fully-resolved map of everything except the 200 stored user patches
const flat = [];
const walk = (arr, base, prefix) => {
  for (const e of arr) {
    const abs = base + e.addr;
    const p = prefix ? `${prefix}.${e.name}` : e.name;
    if ('child' in e) walk(e.child, abs, p);
    else flat.push({ addr: hex7(abs), path: p, desc: e.desc, ...encoding(e.size), min: e.min, max: e.max, init: e.init, ofs: e.ofs, factor: e.factor });
  }
};
walk(am.root.filter((e) => !/^User_patch/.test(e.name)), 0, '');

// ---------------------------------------------------------------- commands
run(read('businesslogic/address_const.js'), 'address_const.js');
const AC = ctx.ADDRESS_CONST;
const hexMap = (o) => Object.fromEntries(Object.entries(o).map(([k, v]) => [k, hexPacked(v)]));
const commands = {
  identityRequest: AC.IDENTITY_REQUEST.match(/../g).join(' '),
  identityReplyPrefix: AC.PART_OF_IDENTITY_REPLY.match(/../g).join(' '),
  commands: hexMap(AC.COMMAND),
  addresses: hexMap(AC.ADDRESS),
  sizes: AC.SIZE,
  enums: { RUNNING_MODE: AC.RUNNING_MODE, EDITOR_COMMUNICATION_MODE: AC.EDITOR_COMMUNICATION_MODE, IS_RQ1_ACCEPT: AC.IS_RQ1_ACCEPT },
};

// --------------------------------------------------------------- resources
run(read('config/resource.js'), 'resource.js');
const resources = vm.runInContext('Resource()', ctx).map((r, i) => ({
  id: i, values: Array.isArray(r.text) ? r.text : String(r.text).split(',').map((s) => s.trim()), ...(r.icon ? { icon: r.icon } : {}),
}));

// ------------------------------------------------------- effect parameters
const ccSrc = read('businesslogic/chain/chain_controller.js');
const fxLists = [...ccSrc.matchAll(/RESOURCE_FX_TYPE\s*=\s*\{\s*text:\s*"([^"]+)"/g)].map((m) => m[1].split(',').map((s) => s.trim()));
const fxTypes = product.communicationLevel >= 3 ? fxLists[fxLists.length - 1] : fxLists[0];
ctx.MAP_BLOCK_TYPE = fxTypes;
let epSrc = read('config/effect_parameter.js');
epSrc = epSrc.replace(/^appendBpmMidiAfterBpm\(\);/m, '/* skipped */');
run(read('businesslogic/numpad_const.js'), 'numpad_const.js');
run(epSrc, 'effect_parameter.js');
const EP = ctx.EFFECT_PARAMETERS;
const effects = fxTypes.map((t, typeId) => {
  const def = EP[t];
  return {
    typeId, name: t,
    parameters: def && def.parameters ? def.parameters.map((p) => ({
      name: p.name, uniqueName: p.uniqueName,
      ...(p.address >= 3 ? { paramIndex: (p.address - 3) / 4 } : { field: p.address === 1 ? 'Switch' : p.address === 0 ? 'Type' : 'DuplicationNumber' }), offsetInFxItem: hex7(p.address),
      ...encoding(p.size), min: p.min, max: p.max, init: p.init, ofs: p.ofs,
      ...(p.factor ? { factor: p.factor } : {}), ...(p.format ? { format: p.format } : {}),
      ...(p.resourceId !== undefined ? { values: resources[p.resourceId]?.values, resourceId: p.resourceId } : {}),
      template: p.templateValue,
    })) : null,
  };
});
const extraEffects = Object.keys(EP).filter((k) => !fxTypes.includes(k));

// ------------------------------------------------------------------ write
fs.mkdirSync(outDir, { recursive: true });
const out = (f, d) => { fs.writeFileSync(path.join(outDir, f), JSON.stringify(d, null, 2) + '\n'); console.log(`wrote ${f}`); };
out('product.json', product);
out('address_map.blocks.json', { root, blocks });
out('address_map.flat.json', flat);
out('commands.json', commands);
out('effects.json', { fxTypes, effects, nonFxBlocks: Object.fromEntries(extraEffects.map((k) => [k, EP[k].parameters || null])) });
out('resources.json', resources);
console.log(`\n${Object.keys(blocks).length} blocks, ${root.length} root entries, ${flat.length} resolved params (system+temp), ${fxTypes.length} fx types`);

// ------------------------------------------------------------ rust tables
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
execFileSync(process.execPath, [fileURLToPath(new URL('./gen_rust.mjs', import.meta.url)), outDir], { stdio: 'inherit' });
