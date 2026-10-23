// Reads the GX-100 parameter tables out of an installed "BOSS TONE STUDIO for GX-100".
//
// A plain script (no imports), shared by two hosts: tools/extract/extract.mjs (Node, for development)
// and the opengx100-extract crate (embedded JavaScript engine, at the editor's first launch). It only defines
// `extractTables(host)`; the host provides:
//   host.read(rel)       text of a file under Contents/Resources/html/ (e.g. 'js/config/address_map.js')
//   host.run(src, name)  run one of the official editor's scripts in a sandbox (shared global scope)
//   host.global          that sandbox's global object
//   host.log(msg)        progress messages
// The result is derived from Roland's files: keep it local, never publish it.

function extractTables(host) {
  const read = (rel) => host.read(rel).replace(/^﻿/, '');
  const js = (rel) => read('js/' + rel);
  const ctx = host.global;
  const run = (src, name) => host.run(src, name);
  ctx.window = ctx;
  run(js('utilities/constant.js'), 'constant.js');
  run(js('utilities/converter.js'), 'converter.js');

  // ------------------------------------------------------- product settings
  const ps = js('config/product_setting.js');
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
  for (const k of ['INTEGER1x1', 'INTEGER1x2', 'INTEGER1x3', 'INTEGER1x4', 'INTEGER1x5', 'INTEGER1x6', 'INTEGER1x7',
    'INTEGER2x4', 'INTEGER2x7', 'INTEGER4x4'])
    ENC[ctx[k]] = k;
  const BYTES = { INTEGER2x4: 2, INTEGER2x7: 2, INTEGER4x4: 4 };
  const encoding = (size) => {
    if (ENC[size]) return { encoding: ENC[size], bytes: BYTES[ENC[size]] || 1 };
    if (size & ctx.PADDING) return { encoding: 'PADDING', bytes: size & 0xffff };
    return { encoding: 'BYTES', bytes: size };
  };

  // ------------------------------------------------------------ address map
  let amSrc = js('config/address_map.js');
  const blockNames = [...amSrc.matchAll(/^var (\w+) = \[/gm)].map((m) => m[1]);
  amSrc = amSrc.replace(/^this\.root = \[/m,
    `this.__blocks = {${blockNames.map((n) => `${n}: ${n}`).join(', ')}};\nthis.root = [`);
  run(amSrc, 'address_map.js');
  const am = run('new AddressMap()', 'address_map');
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
  const blockAddr = {}; // "Temp_patch%common" -> linear address, as the UI definition names blocks
  const walk = (arr, base, prefix) => {
    for (const e of arr) {
      const abs = base + e.addr;
      const p = prefix ? `${prefix}.${e.name}` : e.name;
      if ('child' in e) { blockAddr[p.replaceAll('.', '%')] = abs; walk(e.child, abs, p); }
      else flat.push({ addr: hex7(abs), path: p, desc: e.desc, ...encoding(e.size), min: e.min, max: e.max, init: e.init, ofs: e.ofs, factor: e.factor });
    }
  };
  walk(am.root.filter((e) => !/^User_patch/.test(e.name)), 0, '');

  // ---------------------------------------------------------------- commands
  run(js('businesslogic/address_const.js'), 'address_const.js');
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
  run(js('config/resource.js'), 'resource.js');
  const resources = run('Resource()', 'resource').map((r, i) => ({
    id: i, values: Array.isArray(r.text) ? r.text : String(r.text).split(',').map((s) => s.trim()), ...(r.icon ? { icon: r.icon } : {}),
  }));

  // ------------------------------------------------- parameter value labels
  // The editor's UI definition (bg777 = GX-100) binds widgets to parameters ("System_control%29") and to a
  // resource list for their labels. Only lists that exactly cover a parameter's range are kept.
  const elf = JSON.parse(read('bg777-pc.elf'));
  const lin7 = (hex) => hex.split(' ').reduce((a, x) => a * 128 + parseInt(x, 16), 0);
  const pathAt = new Map(flat.map((f) => [lin7(f.addr), f]));
  const paramValues = new Map();
  let skipped = 0;
  // Dials show their labels through a separate "stringer" widget: "<stem>-stringer" labels "<stem>-dial".
  const stringerRes = new Map();
  for (const page of elf.pages) for (const it of page.items || []) {
    if (it.type === 'stringer' && it['resource-id'] !== undefined && /-stringer$/.test(it.id || ''))
      stringerRes.set(it.id.replace(/-stringer$/, '-dial'), it['resource-id']);
  }
  for (const page of elf.pages) for (let it of page.items || []) {
    if (it['resource-id'] === undefined && stringerRes.has(it.id)) it = { ...it, 'resource-id': stringerRes.get(it.id) };
    if (!it['parameter-id'] || it['resource-id'] === undefined) continue;
    const parts = it['parameter-id'].split('%');
    const base = blockAddr[parts.slice(0, -1).join('%')];
    const f = base === undefined ? undefined : pathAt.get(base + Number(parts[parts.length - 1]));
    const labels = resources[+it['resource-id']]?.values;
    if (!f || !labels) { skipped++; continue; }
    // A list must cover the parameter's range exactly (the widget's own range is sometimes one too wide,
    // e.g. the assign SOURCE list).
    if (labels.length !== f.max - f.min + 1 || +it.min !== f.min) { skipped++; continue; }
    const min = f.min;
    const prev = paramValues.get(f.path);
    if (prev && prev.labels.join() !== labels.join()) { host.log(`labels differ for ${f.path}; keeping the first`); continue; }
    paramValues.set(f.path, { path: f.path, first: min, labels });
  }
  // The editor shows all 20 assigns on one page bound to assign(1); the same labels apply to each.
  for (const v of [...paramValues.values()].filter((v) => v.path.startsWith('Temp_patch.assign(1).'))) {
    for (let n = 2; n <= 20; n++) {
      const path = v.path.replace('assign(1)', `assign(${n})`);
      if (flat.some((f) => f.path === path)) paramValues.set(path, { ...v, path });
    }
  }
  host.log(`value labels for ${paramValues.size} parameters (${skipped} widgets without a usable list)`);

  // ------------------------------------------------------- effect parameters
  const ccSrc = js('businesslogic/chain/chain_controller.js');
  const fxLists = [...ccSrc.matchAll(/RESOURCE_FX_TYPE\s*=\s*\{\s*text:\s*"([^"]+)"/g)].map((m) => m[1].split(',').map((s) => s.trim()));
  const fxTypes = product.communicationLevel >= 3 ? fxLists[fxLists.length - 1] : fxLists[0];
  ctx.MAP_BLOCK_TYPE = fxTypes;
  let epSrc = js('config/effect_parameter.js');
  epSrc = epSrc.replace(/^appendBpmMidiAfterBpm\(\);/m, '/* skipped */');
  run(js('businesslogic/numpad_const.js'), 'numpad_const.js');
  run(epSrc, 'effect_parameter.js');
  const EP = ctx.EFFECT_PARAMETERS;
  // indexMachineValue (a few TYPE lists): the list position of each stored value, when the list
  // isn't in value order. Label of stored value v = values[indexMachineValue[v]].
  const fxParams = (def) => def && def.parameters ? def.parameters.filter((p) => p.address !== undefined).map((p) => ({
    name: p.name, uniqueName: p.uniqueName,
    ...(p.address >= 3 ? { paramIndex: (p.address - 3) / 4 } : { field: p.address === 1 ? 'Switch' : p.address === 0 ? 'Type' : 'DuplicationNumber' }), offsetInFxItem: hex7(p.address),
    ...encoding(p.size), min: p.min, max: p.max, init: p.init, ofs: p.ofs,
    ...(p.factor ? { factor: p.factor } : {}), ...(p.format ? { format: p.format } : {}),
    ...(p.resourceId !== undefined ? { values: resources[p.resourceId]?.values, resourceId: p.resourceId } : {}),
    ...(p.indexMachineValue ? { indexMachineValue: p.indexMachineValue } : {}),
    template: p.templateValue,
  })) : null;
  // Block colours (chain_controller.js / ChainCell.updateColor): each type names a colour; the block image
  // blockImages/EFFECTS_<COLOR>.svg is a hexagon filled with it (its last polygon is the block's face).
  const colors = {};
  const colorOf = (def) => {
    const name = (def && def.color ? def.color : 'black').toUpperCase();
    if (!(name in colors)) {
      let svg = null;
      try { svg = read(`blockImages/EFFECTS_${name}.svg`); } catch (e) { /* no image for this colour */ }
      const fills = svg ? [...svg.matchAll(/<polygon[^>]*fill="(#[0-9A-Fa-f]{6})"/g)].map((m) => m[1].toUpperCase()) : [];
      colors[name] = fills.length ? fills[fills.length - 1] : null;
    }
    return colors[name] ? { color: colors[name] } : {};
  };
  const effects = fxTypes.map((t, typeId) => ({ typeId, name: t, ...colorOf(EP[t]), parameters: fxParams(EP[t]) }));
  // The patch's MASTER settings, as assign targets see them.
  const master = { typeId: 99, name: 'MASTER', parameters: fxParams(EP.MASTER) };

  // ------------------------------------------------------------------ assigns
  // Assign targets: TARGET is an index into ASSIGN_TARGETS; an effect target names a parameter
  // (uniqueName) of the FX in slot TARGET_FX_ITEM, whose type must match the target's category.
  run(js('businesslogic/assign_const.js'), 'assign_const.js');
  const assign = {
    categories: ctx.ASSIGN_CATEGORIES.map((c, i) => {
      if (c.index !== i) throw new Error(`assign category ${c.name} out of order`);
      return { name: c.name, hasSwitch: !!c.hasSwitch, blockName: c.blockName };
    }),
    targets: ctx.ASSIGN_TARGETS.map((t, i) => {
      if (t.index !== i) throw new Error(`assign target ${t.name} out of order`);
      return { name: t.name, category: t.category, effect: t.effect !== false, uniqueName: t.paramUniqueName };
    }),
  };
  const extraEffects = Object.keys(EP).filter((k) => !fxTypes.includes(k));
  host.log(`${Object.keys(blocks).length} blocks, ${root.length} root entries, ${flat.length} resolved params (system+temp), ${fxTypes.length} fx types`);

  return {
    product,
    blocks: { root, blocks },
    flat,
    commands,
    effects: { fxTypes, effects, master, nonFxBlocks: Object.fromEntries(extraEffects.map((k) => [k, EP[k].parameters || null])) },
    assign,
    resources,
    paramValues: [...paramValues.values()].sort((a, b) => (a.path < b.path ? -1 : 1)),
  };
}

// What the editor loads (crates/opengx100-protocol/src/tables/load.rs): the parts of extractTables' result
// it needs, in one object.
function tablesBundle(t) {
  return {
    format: 2,
    product: t.product,
    commands: { commands: t.commands.commands, addresses: t.commands.addresses },
    root: t.blocks.root,
    params: t.flat,
    paramValues: t.paramValues,
    effects: t.effects.effects,
    master: t.effects.master,
    assign: t.assign,
  };
}
