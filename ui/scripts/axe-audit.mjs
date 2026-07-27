// Axe audit across the five routes (M1/M5 exit criterion).
// Usage: node scripts/axe-audit.mjs [baseUrl]
import { chromium } from 'playwright-core';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const axeSource = readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8');

const base = process.argv[2] ?? 'http://localhost:3001';
const routes = ['/pages', '/pages?q=kant', '/connectors', '/connectors/5', '/activity', '/stats', '/prune', '/prune?tab=staged', '/prune?tab=rules', '/prune?tab=history'];

const browser = await chromium.launch({ channel: 'chrome', headless: true, args: ['--no-sandbox'] });
const page = await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage();

let totalViolations = 0;
for (const route of routes) {
  await page.goto(base + route, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(600);
  await page.evaluate(axeSource);
  const results = await page.evaluate(async () => {
    // eslint-disable-next-line no-undef
    return await axe.run(document, {
      runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] },
    });
  });
  console.log(`\n=== ${route} — ${results.violations.length} violations`);
  for (const v of results.violations) {
    totalViolations += 1;
    console.log(`  [${v.impact}] ${v.id}: ${v.help}`);
    for (const node of v.nodes.slice(0, 3)) {
      console.log(`     ${node.target.join(' ')}`);
    }
  }
}
await browser.close();
console.log(`\ntotal: ${totalViolations} violations`);
process.exit(totalViolations > 0 ? 1 : 0);
