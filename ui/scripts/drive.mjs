// Headless-Chrome driver for eyeballing the OVIS UI during the rebuild.
// Usage: node drive.mjs <url> <outPrefix> [width height] [script]
// script: semicolon-separated ops: click=<sel> | fill=<sel>=<text> | press=<key> | wait=<ms> | waitfor=<sel> | shot=<name>
import { chromium } from 'playwright-core';

const [url, prefix = 'shot', width = '1440', height = '900', script = ''] = process.argv.slice(2);
const outDir = new URL('./shots/', import.meta.url).pathname;
import { mkdirSync } from 'node:fs';
mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch({
  channel: 'chrome',
  headless: true,
  args: ['--no-sandbox'],
});
const page = await (await browser.newContext({
  viewport: { width: Number(width), height: Number(height) },
  deviceScaleFactor: 2,
})).newPage();

const errors = [];
page.on('console', (msg) => {
  if (msg.type() === 'error') errors.push(msg.text());
});
page.on('pageerror', (err) => errors.push(String(err)));

await page.goto(url, { waitUntil: 'networkidle', timeout: 30000 });
await page.waitForTimeout(400);

let shotIndex = 0;
async function shot(name) {
  const file = `${outDir}${prefix}-${name ?? shotIndex++}.png`;
  await page.screenshot({ path: file, fullPage: false });
  console.log('SHOT', file);
}

for (const op of script.split(';').map((s) => s.trim()).filter(Boolean)) {
  const [cmd, ...rest] = op.split('=');
  const arg = rest.join('=');
  if (cmd === 'click') await page.click(arg, { timeout: 5000 });
  else if (cmd === 'fill') {
    const i = arg.indexOf('=');
    await page.fill(arg.slice(0, i), arg.slice(i + 1), { timeout: 5000 });
  } else if (cmd === 'press') await page.keyboard.press(arg);
  else if (cmd === 'wait') await page.waitForTimeout(Number(arg));
  else if (cmd === 'waitfor') await page.waitForSelector(arg, { timeout: 10000 });
  else if (cmd === 'shot') await shot(arg);
  else if (cmd === 'hover') await page.hover(arg, { timeout: 5000 });
}
await shot('final');

if (errors.length) {
  console.log('CONSOLE ERRORS:');
  for (const e of errors) console.log('  ' + e.slice(0, 300));
} else {
  console.log('no console errors');
}
await browser.close();
