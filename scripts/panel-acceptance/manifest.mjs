// Write MANIFEST.md for one results directory from its results*.json files.
// Usage: node manifest.mjs <results dir> <results.json> [more ...]
// A later file's check replaces an earlier one's with the same id; a file
// whose name contains "e2e" is listed as its own row ("<id> (e2e build)").
// findings.md in the same directory, if present, is appended.
import fs from 'node:fs';
import path from 'node:path';

const dir = process.argv[2];
const files = process.argv.slice(3);
const runs = files.map(f => ({ file: f, ...JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8')) }));
const rows = new Map();
for (const r of runs) for (const c of r.checks) {
  const id = r.file.includes('e2e') ? `${c.id} (e2e build)` : String(c.id);
  rows.set(id, { ...c, file: r.file, hostSha256: r.hostSha256, buildCommit: r.buildCommit });
}
const order = [...Array.from({ length: 20 }, (_, i) => [String(i + 1), `${i + 1} (e2e build)`]).flat(), 'R'];
const main = runs[0];
const binaries = [...new Map(runs.map(r => [r.hostSha256, r])).values()];
const lines = [
  '# Verifiable panel turns: live acceptance (plan 14.2)', '',
  `Build host 192.168.0.190, ${main.startedAt.slice(0, 10)}. Driver, fixtures and method: \`scripts/panel-acceptance/README.md\`.`, '',
  '| item | value |', '|---|---|',
  ...binaries.map(r => `| host binary (\`${r.buildCommit}\`, debug profile) sha256 | \`${r.hostSha256}\` |`),
  `| extension tree hash (extensions/ab-connect ${main.extensionVersion}, tests excluded) | \`${main.extensionTree}\` |`,
  `| Chrome for Testing | ${main.chrome} (headless=new, private --user-data-dir) |`,
  `| v1.5.214 release binary sha256 (checks 15, 16) | \`${main.v15214Sha256 || 'n/a'}\` |`,
  `| v1.5.214 extension tree hash (checks 15) | \`${main.v15214ExtensionTree || 'n/a'}\` |`,
  `| result files | ${runs.map(r => `\`${r.file}\` (${r.startedAt})`).join(', ')} |`,
  '', '| check | name | status | seconds | from | note / error |', '|---|---|---|---|---|---|',
];
for (const id of order) {
  const c = rows.get(id);
  if (!c) { if (!id.includes('e2e')) lines.push(`| ${id} | | not run | | | |`); continue; }
  const msg = (c.error || c.note || '').split('\n')[0].replace(/\|/g, '\\|').slice(0, 400);
  lines.push(`| ${id} | ${c.name} | ${c.status} | ${c.seconds} | ${c.file} | ${msg} |`);
}
lines.push('', 'Evidence per check: `checks[].evidence` in the result files. Side-panel screenshots (`panel-*.png`, the real side panel, not a tab) are not committed (`*.png` is git-ignored); they stay next to the results on the build host (`~/panel-accept/repo/scripts/panel-acceptance/results/<date>/`).');
const findings = path.join(dir, 'findings.md');
if (fs.existsSync(findings)) lines.push('', fs.readFileSync(findings, 'utf8').trim());
fs.writeFileSync(path.join(dir, 'MANIFEST.md'), lines.join('\n') + '\n');
console.log(lines.join('\n'));
