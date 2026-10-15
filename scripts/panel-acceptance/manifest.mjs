// Write MANIFEST.md next to a results.json (or several merged ones).
// Usage: node manifest.mjs <results dir> [results-*.json ...]
import fs from 'node:fs';
import path from 'node:path';
const dir = process.argv[2];
const files = process.argv.length > 3 ? process.argv.slice(3) : ['results.json'];
const runs = files.map(f => JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8')));
const latest = new Map();
for (const r of runs) for (const c of r.checks) latest.set(String(c.id), { ...c, run: r.startedAt });
const order = [...Array.from({ length: 20 }, (_, i) => String(i + 1)), 'R'];
const head = runs.at(-1);
const lines = [
  '# Verifiable panel turns: live acceptance (plan 14.2)', '',
  `Run on the build host (192.168.0.190), ${head.startedAt}.`, '',
  '| item | value |', '|---|---|',
  `| branch commit | ${head.buildCommit || 'n/a'} |`,
  `| host binary sha256 | \`${head.hostSha256}\` |`,
  `| extension tree hash (ab-connect, ${head.extensionVersion}) | \`${head.extensionTree}\` |`,
  `| Chrome for Testing | ${head.chrome} (headless=new, own --user-data-dir) |`,
  `| v1.5.214 binary sha256 (checks 15, 16) | \`${head.v15214Sha256 || 'n/a'}\` |`,
  `| v1.5.214 extension tree hash | \`${head.v15214ExtensionTree || 'n/a'}\` |`,
  '', '| check | name | status | seconds | note / error |', '|---|---|---|---|---|',
];
for (const id of order) {
  const c = latest.get(id);
  if (!c) { lines.push(`| ${id} | | not run | | |`); continue; }
  const msg = (c.error || c.note || '').split('\n')[0].replace(/\|/g, '\\|').slice(0, 300);
  lines.push(`| ${id} | ${c.name} | ${c.status} | ${c.seconds} | ${msg} |`);
}
lines.push('', 'Evidence per check is in results.json (`checks[].evidence`); side-panel screenshots are panel-*.png.');
fs.writeFileSync(path.join(dir, 'MANIFEST.md'), lines.join('\n') + '\n');
console.log(lines.join('\n'));
