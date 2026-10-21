#!/usr/bin/env node
// Fetches the GX-100 parameter guide (Roland's online manual) and turns it into descriptions for the
// editor: what each effect does (and what it models), each parameter, each list option, and the
// patch/system settings. The text is Roland's: it goes to ./extracted (gitignored) like the tables,
// and must not be published. Run after extract.mjs; it regenerates extracted/tables.rs.
//
// usage: node tools/extract/manual.mjs [--refresh] [--dump]
//   --refresh  download the pages again (they're cached in extracted/manual/)
//   --dump     print what was parsed from each page, and what couldn't be matched

import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { SETTINGS } from './manual_map.mjs';

const BASE = 'https://static.roland.com/manuals/gx-100_parameter_v200/en-US/';
const outDir = path.resolve('extracted');
const cache = path.join(outDir, 'manual');
const refresh = process.argv.includes('--refresh');
const dump = process.argv.includes('--dump');

// ------------------------------------------------------------------ fetch
async function page(file) {
  const f = path.join(cache, file);
  if (!refresh && fs.existsSync(f)) return fs.readFileSync(f, 'utf8');
  const res = await fetch(BASE + file);
  if (!res.ok) { console.warn(`skipping ${file}: HTTP ${res.status}`); return ''; }
  const html = await res.text();
  fs.mkdirSync(cache, { recursive: true });
  fs.writeFileSync(f, html);
  return html;
}

// ------------------------------------------------------------------ html → text
// Symbols drawn with Roland's symbol font.
const GLYPHS = { 'Ç': '▼', 'à': '▼', 'É': '▲', 'á': '▲', 'Ø': '→', '÷': '◀', 'ø': '▶', 'ŀ': '𝅝', 'Ō': '♬' };
const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };
const decode = (s) => s
  .replace(/<span class="FontRoland\d+">([^<]*)<\/span>/g, (_, g) => [...g].map((c) => GLYPHS[c] ?? '').join(''))
  .replace(/<br\s*\/?>/g, '\n')
  .replace(/<\/p>/g, '\n')
  .replace(/<[^>]+>/g, '')
  .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(+n))
  .replace(/&#x([0-9a-f]+);/gi, (_, n) => String.fromCodePoint(parseInt(n, 16)))
  .replace(/&(\w+);/g, (m, e) => ENTITIES[e] ?? m);
const clean = (s) => decode(s).split('\n').map((l) => l.replace(/\s+/g, ' ').trim()).filter(Boolean).join('\n');

// A table as rows of cell texts, with rowspan/colspan cells repeated where they reach.
function grid(tableHtml) {
  const rows = [];
  const pending = []; // [col] = { text, left }
  const body = tableHtml.replace(/<thead>[\s\S]*?<\/thead>/, '');
  for (const tr of body.match(/<tr[\s\S]*?<\/tr>/g) || []) {
    const row = [];
    const cells = [...tr.matchAll(/<td([^>]*)>([\s\S]*?)<\/td>/g)];
    let col = 0;
    const fillPending = () => {
      while (pending[col] && pending[col].left > 0) { row[col] = pending[col].text; pending[col].left--; col++; }
    };
    for (const [, attrs, inner] of cells) {
      fillPending();
      const rs = +(/rowspan="(\d+)"/.exec(attrs)?.[1] || 1);
      const cs = +(/colspan="(\d+)"/.exec(attrs)?.[1] || 1);
      const text = clean(inner);
      for (let k = 0; k < cs; k++) {
        row[col] = text;
        if (rs > 1) pending[col] = { text, left: rs - 1 };
        col++;
      }
    }
    fillPending();
    rows.push(row);
  }
  return rows;
}

// Each column's role from the header: "Parameter", "Value" or "Explanation" (headers use colspan).
const columnRoles = (tableHtml) => {
  const roles = [];
  for (const th of (/<thead>([\s\S]*?)<\/thead>/.exec(tableHtml)?.[1] || '').match(/<th[\s\S]*?<\/th>/g) || []) {
    const cs = +(/colspan="(\d+)"/.exec(th)?.[1] || 1);
    for (let k = 0; k < cs; k++) roles.push(clean(th));
  }
  return roles;
};

const uniq = (xs) => xs.filter((x, i) => x && xs.indexOf(x) === i);

// One page: its intro text and what it says about each parameter.
function parsePage(html) {
  const article = /<article[\s\S]*?<\/article>/.exec(html)?.[0] || '';
  const title = clean(/<h1[^>]*>([\s\S]*?)<\/h1>/.exec(article)?.[1] || '');
  const tables = [...article.matchAll(/<table[\s\S]*?<\/table>/g)]
    .map((m) => ({ html: m[0], index: m.index, roles: columnRoles(m[0]) }));
  const isParams = (t) => t.roles.includes('Explanation');
  // Intro: the paragraphs before the first parameter table (not the "Chain / Palette" picture table).
  const first = tables.find(isParams);
  const introHtml = article.slice(0, first ? first.index : article.length)
    .replace(/<h1[\s\S]*?<\/h1>/, '').replace(/<table[\s\S]*?<\/table>/g, '');
  const intro = [...introHtml.matchAll(/<p[^>]*>([\s\S]*?)<\/p>/g)].map((m) => clean(m[1])).filter(Boolean)
    .filter((l) => !/^(Chain|Palette|MONO\/STEREO|MONO|STEREO)$/.test(l)).join('\n');

  // Rows grouped by parameter name, in page order: [{ name, rows: [{ value, text }] }]
  const groups = [];
  for (const t of tables.filter(isParams)) {
    for (const row of grid(t.html)) {
      const pick = (role) => uniq(t.roles.map((r, i) => (r === role ? row[i] : undefined)));
      const name = pick('Parameter').join(' ');
      const explanation = pick('Explanation');
      // A cell spanning Value and Explanation is about the parameter itself.
      const values = pick('Value').filter((v) => !explanation.includes(v));
      const value = values.join('\n');
      const text = explanation.join('\n');
      if (!text && !value) continue;
      let g = groups.find((x) => x.name === name);
      if (!g) groups.push(g = { name, rows: [] });
      g.rows.push({ value, text });
    }
  }
  const params = groups.map(({ name, rows }) => {
    const p = { name, text: '', range: '', options: [] };
    const addText = (t) => { if (t && !p.text.split('\n').includes(t)) p.text = p.text ? `${p.text}\n${t}` : t; };
    const valued = rows.filter((r) => r.value);
    for (const r of rows) {
      if (!r.value) addText(r.text);
      else if (valued.length === 1 || isNumeric(r.value)) { p.range = r.value; addText(r.text); }
      else for (const label of expandLabels(r.value.split('\n')[0])) p.options.push([label, r.text]);
    }
    // Options that all say the same thing are really one explanation of the parameter.
    if (p.options.length > 1 && p.options.every(([, t]) => t === p.options[0][1])) {
      addText(p.options[0][1]);
      p.range = p.options.map(([l]) => l).join(', ');
      p.options = [];
    }
    return p;
  });
  return { title, intro, params };
}

// "0–120", "-50–+50", "0.0 ms–40.0 ms", "1 cm–10 cm": a span of numbers, not an option.
const isNumeric = (v) => /^[-+]?\d[\d.]*\s*[a-zA-Z%]*\s*[–-]/.test(v);

// An option row can stand for several values: "CTL 1, CTL 2", "NUM 1–NUM 4", "CC# 1–31, 64–95".
function expandLabels(label) {
  const out = [];
  let prefix = '';
  for (const part of label.split(/,\s*|、/).map((s) => s.trim()).filter(Boolean)) {
    const m = /^(.*?)(\d+)\s*[–～]\s*(\D*?)(\d+)$/.exec(part);
    if (m && (!m[3] || m[3].trim() === m[1].trim()) && +m[4] > +m[2] && +m[4] - +m[2] < 200) {
      if (m[1]) prefix = m[1];
      for (let i = +m[2]; i <= +m[4]; i++) out.push(`${prefix}${i}`);
    } else {
      out.push(part);
    }
  }
  return out;
}

// ------------------------------------------------------------------ matching
const norm = (s) => String(s).toUpperCase().replace(/[^A-Z0-9]/g, '');
// The guide names a few parameters differently from the editor's tables.
const NAME_ALIASES = { ONOFF: ['OFFON'], OFFON: ['ONOFF'], FILTER: ['FILTERMODE'], AMPCTL1: ['AMPCTL1'] };
const sameName = (a, b) => {
  const [x, y] = [norm(a), norm(b)];
  return x === y || (NAME_ALIASES[x] || []).includes(y);
};
// "AMP CTL1, AMP CTL2", "1: RATE\n2: RATE": one row about several parameters. Footnote marks (*1) go.
const splitNames = (n) => n.replace(/\s*\*\d+/g, '').split(/\n|,\s*/).map((s) => s.trim()).filter(Boolean);

function docOf(p) {
  const options = p.options.filter(([l, t]) => l && t && !/^\[img\]$/.test(t));
  return { text: p.text, options };
}

async function main() {
  const index = await page('index.html');
  const links = [...index.matchAll(/<a[^>]*href="(\d+\.html)"[^>]*>([\s\S]*?)<\/a>/g)]
    .map((m) => ({ file: m[1], title: clean(m[2]) }))
    .filter((l, i, all) => all.findIndex((x) => x.file === l.file) === i);
  const pages = [];
  for (const l of links) {
    const html = await page(l.file);
    if (!html) continue;
    pages.push({ ...parsePage(html), file: l.file, indexTitle: l.title });
  }
  console.log(`${pages.length} pages of the parameter guide`);

  const { effects, master } = JSON.parse(fs.readFileSync(path.join(outDir, 'effects.json'), 'utf8'));
  const flat = JSON.parse(fs.readFileSync(path.join(outDir, 'address_map.flat.json'), 'utf8'));
  const unmatched = [];

  // Effects: a page per FX type (DIVIDER and MIXER share one). "<PARAM> list" pages and sub-pages
  // ("TYPE: DUAL parameters", "USER SCALE settings") belong to the effect page before them.
  const fxPages = new Map(); // fx name -> [pages]
  let last = null;
  for (const p of pages) {
    const names = p.title.split(/[／/]/).map((s) => s.trim());
    const fxs = [...effects, master].filter((fx) => names.some((n) => sameName(n, fx.name)) || sameName(p.title, fx.name));
    if (fxs.length) {
      last = fxs;
      for (const fx of fxs) fxPages.set(fx.name, [...(fxPages.get(fx.name) || []), { ...p, main: true }]);
    } else if (last && /( list| parameters| settings)$/.test(p.title) && !/^TARGET list$/.test(p.title)) {
      for (const fx of last) fxPages.get(fx.name).push({ ...p, main: false });
    } else {
      last = null;
    }
  }

  const fxDocs = [];
  for (const fx of [...effects, master]) {
    const ps = fxPages.get(fx.name);
    if (!ps || !fx.parameters) { if (dump) unmatched.push(`effect without a page: ${fx.name}`); continue; }
    const text = ps.filter((p) => p.main).map((p) => p.intro).filter(Boolean).join('\n');
    const params = [];
    const add = (fp, doc) => {
      let d = params.find((x) => x.key === fp.uniqueName);
      if (!d) params.push(d = { key: fp.uniqueName, text: '', options: [] });
      if (doc.text && !d.text.includes(doc.text)) d.text = d.text ? `${d.text}\n${doc.text}` : doc.text;
      for (const o of doc.options) if (!d.options.some(([l]) => norm(l) === norm(o[0]))) d.options.push(o);
    };
    for (const p of ps) {
      for (const mp of p.params) {
        // A "… list" page holds the options of the parameter in its title.
        const name = mp.name || p.title.replace(/ list$/, '').replace(new RegExp(`^${fx.name}\\s+`), '');
        const targets = fx.parameters.filter((fp) => splitNames(name).some((n) => sameName(n, fp.name)) || sameName(name, fp.name));
        if (!targets.length) { if (dump && p.main) unmatched.push(`${fx.name}: guide parameter "${name}"`); continue; }
        for (const fp of targets) add(fp, docOf(mp));
      }
    }
    fxDocs.push({ fx: fx.name, text, params: params.filter((d) => d.text || d.options.length) });
  }

  // Patch and system settings: SETTINGS maps (guide page, parameter name) to our parameter paths.
  const byTitle = new Map(pages.map((p) => [norm(p.title), p]));
  const known = new Set(flat.map((f) => f.path.replace(/\((\d+)\)/g, '(n)')));
  const paramDocs = new Map();
  for (const [title, entries] of Object.entries(SETTINGS)) {
    const p = byTitle.get(norm(title));
    if (!p) { unmatched.push(`settings page not found: ${title}`); continue; }
    for (const [name, paths] of entries) {
      const bare = (n) => n.replace(/\s*\*\d+/g, '').replace(/^\[\d\]\s*/, '');
      // '' with no unnamed table: the page's text, its rows being the options ("MEMORY (Memory mode)").
      const rowsAsOptions = () => ({ text: p.intro, options: p.params.map((x) => [x.name.split('\n')[0], x.text]) });
      const mp = p.params.find((x) => sameName(bare(x.name), bare(name))) || (name === '' && rowsAsOptions());
      if (!mp) { unmatched.push(`${title}: no row "${name}"`); continue; }
      for (const key of [paths].flat()) {
        if (!known.has(key)) { unmatched.push(`${title}: unknown parameter ${key}`); continue; }
        const doc = docOf(mp);
        const text = [name === '' && mp.text !== p.intro ? p.intro : '', doc.text].filter(Boolean).join('\n');
        paramDocs.set(key, { key, text, options: doc.options });
      }
    }
  }

  if (dump) {
    for (const p of pages) {
      console.log(`\n== ${p.title} (${p.file})${p.intro ? `\n   intro: ${p.intro.slice(0, 160).replace(/\n/g, ' / ')}` : ''}`);
      for (const x of p.params) console.log(`   [${x.name}] ${x.range} :: ${x.text.slice(0, 80).replace(/\n/g, ' / ')}${x.options.length ? ` {${x.options.map((o) => o[0]).join(' | ')}}` : ''}`);
    }
    console.log('\nunmatched:\n  ' + unmatched.join('\n  '));
  } else if (unmatched.length) {
    console.warn(`${unmatched.length} guide entries not matched (run with --dump to list them)`);
  }

  const docs = { fx: fxDocs, params: [...paramDocs.values()] };
  fs.writeFileSync(path.join(outDir, 'docs.json'), JSON.stringify(docs, null, 2) + '\n');
  const nParams = fxDocs.reduce((a, f) => a + f.params.length, 0);
  console.log(`wrote docs.json: ${fxDocs.length} effects (${nParams} parameters), ${paramDocs.size} settings`);
  execFileSync(process.execPath, [fileURLToPath(new URL('./gen_rust.mjs', import.meta.url)), outDir], { stdio: 'inherit' });
}

main().catch((e) => { console.error(e); process.exit(1); });
