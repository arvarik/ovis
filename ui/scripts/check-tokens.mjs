// Token discipline check (replaces eslint-plugin-tailwindcss, which does not
// understand Tailwind v4 CSS-first config). Fails the build when a component
// reaches outside the design tokens: raw Tailwind palette scales (gray-800,
// emerald-500, …) or hex colors in class strings. The palette itself lives in
// src/styles/theme.css, which is the one file allowed to say #hex.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const ROOT = new URL('../src', import.meta.url).pathname;
const ALLOW_FILES = new Set(['styles/theme.css']);

// e.g. bg-gray-800, text-emerald-400/70, border-slate-200 — any scale-number
// suffix means the default palette leaked past the tokens.
const RAW_SCALE =
  /\b(?:bg|text|border|ring|outline|fill|stroke|divide|decoration|shadow|accent|caret|from|via|to)-(?:gray|slate|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}\b/;
// e.g. bg-[#0A1F13] — hex smuggled through an arbitrary value.
const RAW_HEX = /\[[^\]]*#[0-9a-fA-F]{3,8}[^\]]*\]/;

const failures = [];

function walk(dir) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      walk(path);
      continue;
    }
    if (!/\.(tsx?|css)$/.test(entry)) continue;
    const rel = relative(ROOT, path);
    if (ALLOW_FILES.has(rel)) continue;
    const lines = readFileSync(path, 'utf8').split('\n');
    lines.forEach((line, i) => {
      for (const re of [RAW_SCALE, RAW_HEX]) {
        const m = line.match(re);
        if (m) failures.push(`src/${rel}:${i + 1}  ${m[0]}`);
      }
    });
  }
}

walk(ROOT);

if (failures.length > 0) {
  console.error('off-token color usage (use theme.css tokens instead):');
  for (const f of failures) console.error('  ' + f);
  process.exit(1);
}
console.log('token check clean');
