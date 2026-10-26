import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const benchmark = fileURLToPath(new URL('./stealth-bench.sh', import.meta.url));

function run(locale, gpuFailed = false) {
  const directory = mkdtempSync(path.join(tmpdir(), 'stealth-bench-test-'));
  try {
    const cli = path.join(directory, 'fixture-cli');
    writeFileSync(cli, `#!${process.execPath}
const args=process.argv.slice(2);
if(args.includes('--version')) console.log('fixture chrome-use');
else if(args.includes('open')||args.includes('close')) {}
else if(args.includes('eval')) {
  const expression=args[args.indexOf('eval')+1];
  let value;
  if(expression.includes('new Worker')) value=process.env.AGENT_BROWSER_LOCALE==='ja-JP'?process.env.FIXTURE_LOCALE:'same en-US';
  else if(expression.includes('td.passed')) value=process.env.FIXTURE_GPU_FAILED==='1'?'WebGL Vendor, WebGL Renderer':'clean';
  else if(expression.includes('headless:')) value='0 0';
  else if(expression.includes('new-tests')) value='clean';
  else throw Error('unexpected evaluation: '+expression);
  console.log(JSON.stringify(value));
} else if(args.includes('get')&&args.includes('title')) console.log('Cloudflare Challenge');
else if(args.includes('get')&&args.includes('text')) console.log(Array(12).fill('Normal').join('\\n'));
else throw Error('unexpected command: '+args.join(' '));
`, { mode: 0o700 });
    writeFileSync(path.join(directory, 'sleep'), '#!/bin/sh\nexit 0\n', { mode: 0o700 });
    return spawnSync('bash', [benchmark, '--launch'], {
      env: { ...process.env, PATH: directory + path.delimiter + process.env.PATH,
        CHROME_USE: cli, FIXTURE_LOCALE: locale, FIXTURE_GPU_FAILED: gpuFailed ? '1' : '0' },
      encoding: 'utf8', timeout: 10000,
    });
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

for (const locale of ['same ja-JP', 'same ja-JP,ja']) {
  test(`accepts matching native locale: ${locale}`, () => {
    const result = run(locale);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /locale applied everywhere/);
  });
}

for (const locale of ['same en-US', 'same ja', 'same ja-JP,en-US', 'same ja-JP,ja,ja',
  'main ja-JP / worker ja-JP,ja', '', 'null', 'worker probe failed']) {
  test(`refuses wrong, mismatched or missing locale: ${locale}`, () => {
    const result = run(locale);
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL.*locale applied everywhere/);
  });
}

test('a valid reduced locale does not mask detector failures', () => {
  const result = run('same ja-JP', true);
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stdout, /FAIL.*sannysoft.*WebGL Vendor/);
  assert.match(result.stdout, /PASS.*locale applied everywhere/);
});
