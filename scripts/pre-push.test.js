// Run with: sh scripts/test-pre-push-sandbox.sh. No packages are installed.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

// Fail before running a hook unless the expected container restrictions hold.
assert.equal(process.getuid(), 65534);
const status = readFileSync('/proc/self/status', 'utf8');
assert.match(status, /^CapEff:\s+0+$/m);
assert.match(status, /^NoNewPrivs:\s+1$/m);
assert.match(status, /^Seccomp:\s+2$/m);
assert.deepEqual(Object.keys(networkInterfaces()).filter(name => name !== 'lo'), []);
assert.equal(existsSync('/var/run/docker.sock'), false);
for (const name of ['SSH_AUTH_SOCK', 'DISPLAY', 'WAYLAND_DISPLAY', 'GH_TOKEN', 'GITHUB_TOKEN']) {
  assert.equal(process.env[name], undefined);
}
assert.throws(() => writeFileSync('/tmp-root-write-probe', 'probe'), { code: 'EROFS' });
assert.match(readFileSync('/proc/self/mountinfo', 'utf8'), / \/tmp .* - tmpfs /);
console.log('Sandbox verified: unprivileged, no capabilities, no new privileges, seccomp, no external network interface, read-only root.');

const hook = fileURLToPath(new URL('../.husky/pre-push', import.meta.url));

function git(cwd, args, env = {}) {
  const result = spawnSync('/usr/bin/git', args, {
    cwd, env: { ...process.env, ...env }, encoding: 'utf8', timeout: 10_000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  return result.stdout.trim();
}

function push({ optIn, skip, publisherStatus = '0', publisher = true } = {}) {
  const root = mkdtempSync('/tmp/pre-push-');
  try {
    const work = join(root, 'work');
    const remote = join(root, 'remote.git');
    const bin = join(root, 'bin');
    const log = join(root, 'publisher.log');
    mkdirSync(work);
    mkdirSync(bin);
    // An empty template avoids sample or globally configured hooks.
    git(root, ['init', '--bare', '--template=', remote]);
    git(work, ['init', '--template=', '-b', 'main']);
    git(work, ['config', 'user.name', 'Hook Test']);
    git(work, ['config', 'user.email', 'hook@example.invalid']);
    git(work, ['-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgSign=false', 'commit', '--allow-empty', '-m', 'fixture']);
    mkdirSync(join(work, '.git/hooks'));
    writeFileSync(join(work, '.git/hooks/pre-push'), `#!/bin/sh\necho 'fixture: pre-push entered' >&2\nexec /bin/sh "${hook}" "$@"\n`, { mode: 0o700 });
    if (publisher) {
      writeFileSync(join(bin, 'pnpm'), '#!/bin/sh\nprintf "%s\\n" "$@" >> "$PUBLISH_LOG"\nexit "$PUBLISH_STATUS"\n', { mode: 0o700 });
    }
    const env = {
      // Exclude the image's /usr/local/bin so a real pnpm cannot be selected.
      PATH: `${bin}:/usr/bin:/bin`, PUBLISH_LOG: log, PUBLISH_STATUS: publisherStatus,
    };
    if (optIn !== undefined) env.CHROME_USE_CLAWHUB_SYNC = optIn;
    if (skip !== undefined) env.SKIP_CLAWHUB_SYNC = skip;
    const result = spawnSync('/usr/bin/git', ['push', remote, 'main'], {
      cwd: work, env: { ...process.env, ...env }, encoding: 'utf8', timeout: 10_000,
    });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stderr, /fixture: pre-push entered/);
    // Verify the effect: the remote actually received the expected commit.
    assert.equal(git(root, ['--git-dir', remote, 'rev-parse', 'refs/heads/main']), git(work, ['rev-parse', 'HEAD']));
    return {
      output: result.stdout + result.stderr,
      calls: existsSync(log) ? readFileSync(log, 'utf8') : '',
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test('ordinary pushes do not invoke the publisher', () => {
  assert.equal(push().calls, '');
});

test('only the exact opt-in value 1 enables publishing', () => {
  for (const optIn of ['', '0', 'true', 'yes']) {
    assert.equal(push({ optIn }).calls, '', `unexpected publishing for ${JSON.stringify(optIn)}`);
  }
});

test('opt-in invokes the expected pnpm script exactly once', () => {
  assert.equal(push({ optIn: '1' }).calls, 'run\nclawhub:sync\n');
});

test('the existing skip setting overrides opt-in', () => {
  const result = push({ optIn: '1', skip: '1' });
  assert.equal(result.calls, '');
  assert.match(result.output, /Skipping ClawHub sync/);
  assert.equal(push({ optIn: '1', skip: '0' }).calls, 'run\nclawhub:sync\n');
});

test('publisher failure warns while the Git push still completes', () => {
  const result = push({ optIn: '1', publisherStatus: '23' });
  assert.equal(result.calls, 'run\nclawhub:sync\n');
  assert.match(result.output, /ClawHub sync failed\. Push continues/);
});

test('ordinary pushes do not require pnpm', () => {
  const result = push({ publisher: false });
  assert.equal(result.calls, '');
  assert.doesNotMatch(result.output, /not found|ClawHub sync failed/);
});
