// Live check for `/resume` with a real backend (build host only):
//   node resume-live.mjs <chrome-use binary> codex|opencode
// Creates a throwaway CLI session in a scratch project with the CLI itself,
// starts the panel service in an isolated HOME, lists and resumes that
// session through the service socket (as the panel does), sends one message
// and checks the agent remembers the earlier turn. The CLI's login is copied
// into a private data directory: the user's own sessions are never listed,
// read or written. Evidence goes to <work>/evidence.json.
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { randomUUID } from 'node:crypto';
import { Service, waitFor, sleep } from './lib.mjs';

const [bin, agent] = process.argv.slice(2);
if (!bin || !['codex', 'opencode'].includes(agent)) { console.error('usage: node resume-live.mjs <chrome-use binary> codex|opencode'); process.exit(64); }
const work = fs.realpathSync(fs.mkdtempSync('/tmp/pr-'));
const home = path.join(work, 'home'), project = path.join(work, 'project');
for (const d of [home, project]) fs.mkdirSync(d, { recursive: true });
const word = `${['OSPREY', 'EGRET', 'HERON', 'KITE'][Math.floor(Math.random() * 4)]}-${Math.floor(Math.random() * 90 + 10)}`;
const env = { ...process.env, HOME: home, CHROME_USE_NO_UPDATE_CHECK: '1' };
let executable, model = null;
if (agent === 'codex') {
  env.CODEX_HOME = path.join(work, 'codex');
  fs.mkdirSync(env.CODEX_HOME);
  fs.copyFileSync(path.join(process.env.REAL_CODEX_HOME || path.join(os.homedir(), '.codex'), 'auth.json'), path.join(env.CODEX_HOME, 'auth.json'));
  executable = spawnSync('sh', ['-c', 'command -v codex'], { env: { ...process.env, PATH: `${os.homedir()}/.local/bin:/opt/homebrew/bin:${process.env.PATH}` }, encoding: 'utf8' }).stdout.trim();
} else {
  env.XDG_DATA_HOME = path.join(work, 'xdg'); env.XDG_STATE_HOME = path.join(work, 'xstate');
  fs.mkdirSync(path.join(env.XDG_DATA_HOME, 'opencode'), { recursive: true });
  const auth = path.join(env.XDG_DATA_HOME, 'opencode', 'auth.json');
  fs.copyFileSync(process.env.REAL_OPENCODE_AUTH || path.join(os.homedir(), '.local/share/opencode/auth.json'), auth); fs.chmodSync(auth, 0o600);
  // Its provider settings, copied: the private HOME has none.
  env.XDG_CONFIG_HOME = path.join(work, 'xconfig');
  fs.mkdirSync(path.join(env.XDG_CONFIG_HOME, 'opencode'), { recursive: true });
  const config = path.join(os.homedir(), '.config/opencode/opencode.json');
  if (fs.existsSync(config)) fs.copyFileSync(config, path.join(env.XDG_CONFIG_HOME, 'opencode', 'opencode.json'));
  executable = process.env.REAL_OPENCODE_BIN || '/private/tmp/chrome-use-opencode-2.0.25';
  model = process.env.REAL_OPENCODE_MODEL || 'google/gemini-4-argon';
}
const evidence = { agent, work, project, word, executable, steps: [] };
const step = (name, data) => { evidence.steps.push({ name, ...data }); console.log(`[${agent}] ${name}`, JSON.stringify(data).slice(0, 400)); };
const check = (condition, message) => { if (!condition) { fs.writeFileSync(path.join(work, 'evidence.json'), JSON.stringify(evidence, null, 1)); throw new Error(`${agent}: ${message}`); } };

// 1. A terminal session the panel did not start.
const first = `Remember the codeword ${word}. Reply with just OK.`;
const created = agent === 'codex'
  ? spawnSync(executable, ['exec', '--skip-git-repo-check', '-c', 'model_reasoning_effort=low', first], { cwd: project, env, encoding: 'utf8', timeout: 180000, stdio: ['ignore', 'pipe', 'pipe'] })
  // OpenCode takes its directory from PWD, not the process's working directory.
  : spawnSync(executable, ['run', '--standalone', '-m', model, first], { cwd: project, env: { ...env, PWD: project }, encoding: 'utf8', timeout: 180000, stdio: ['ignore', 'pipe', 'pipe'] });
step('terminal session created', { status: created.status, answer: created.stdout.trim().split('\n').slice(-1)[0] });
check(created.status === 0, 'the CLI could not create the throwaway session');

// 2. The panel service in an isolated HOME.
const profile = `resume-${agent}`;
const root = path.join(home, '.chrome-use', 'agent-panel');
const log = fs.openSync(path.join(work, 'service.log'), 'a');
const proc = spawn(bin, ['__panel-service', '--profile', profile, '--data-dir', root], { env, stdio: ['ignore', log, log], detached: true });
proc.unref();
const service = new Service(path.join(root, 'profiles', profile, 'service.json'));
try {
  await waitFor(async () => (await service.call('health', {})) && true, { timeout: 20000, label: 'service' });

  // 3. List: the throwaway session shows, from its project.
  const listed = await service.call('conversationSessionList', { agentId: agent, executable }, 40000);
  const row = listed.sessions.find(s => s.cwd === project);
  step('listed', { supported: listed.supported, count: listed.sessions.length, row: row ?? listed.sessions });
  check(row, 'the terminal session was not listed');

  // 4. Resume: written seconds ago, so it first needs confirmation.
  const base = { agentId: agent, sessionId: row.sessionId, executable, ...(model ? { model } : {}) };
  const held = await service.call('conversationResume', base, 50000);
  step('resume without confirmation', { confirmRequired: held.confirmRequired ?? null, conversation: held.conversation?.id ?? null });
  const resumed = held.confirmRequired ? await service.call('conversationResume', { ...base, confirmRunning: true }, 50000) : held;
  const id = resumed.conversation.id;
  step('resumed', { conversation: id, resumed: resumed.conversation.resumed, title: resumed.conversation.title, phase: resumed.conversation.phase });
  check(resumed.conversation.resumed?.sessionId === row.sessionId, 'the conversation is not bound to the session');

  // 5. The earlier transcript was imported.
  const before = await service.journal(id);
  const imported = before.events.filter(e => e.kind === 'importedHistory');
  step('imported history', { events: imported.map(e => e.data) });
  check(imported.some(e => String(e.data.text || '').includes(word)), 'the earlier turn was not imported');

  // 6. One message: the agent must remember the earlier turn.
  const messageId = randomUUID();
  await service.call('conversationSend', { conversationId: id, messageId, text: 'What codeword did I ask you to remember? Reply with just the codeword.' }, 30000);
  const ended = await waitFor(async () => {
    const j = await service.journal(id);
    return j.events.find(e => e.kind === 'turnEnded' && e.data.messageId === messageId) && j;
  }, { timeout: 240000, interval: 1000, label: 'the turn to end' });
  // OpenCode only streams its answer (textDelta); the others also send it whole.
  const texts = kind => ended.events.filter(e => e.kind === kind && e.data.messageId === messageId).map(e => e.data.text).join('');
  const answer = texts('textFinal') || texts('textDelta');
  const outcome = ended.events.find(e => e.kind === 'turnEnded' && e.data.messageId === messageId).data.outcome;
  const ready = ended.events.filter(e => e.kind === 'ready').map(e => e.data.sessionId);
  step('answer', { outcome, answer, readySessions: ready });
  check(outcome === 'completed' && answer.includes(word), 'the agent did not remember the earlier turn');
  check(ready.every(s => s === row.sessionId), 'the backend ran another session');

  // 7. The CLI's own session continued; nothing was written in the project.
  if (agent === 'codex') {
    const files = spawnSync('find', [path.join(env.CODEX_HOME, 'sessions'), '-name', `*${row.sessionId}.jsonl`], { encoding: 'utf8' }).stdout.trim().split('\n').filter(Boolean);
    const text = files.map(f => fs.readFileSync(f, 'utf8')).join('');
    step('codex rollout continued', { files: files.length, hasQuestion: text.includes('What codeword did I ask you') });
    check(files.length === 1 && text.includes('What codeword did I ask you'), 'the panel turn is not in the original rollout');
  } else {
    const exported = spawnSync(executable, ['session', 'export', '--standalone', row.sessionId], { cwd: work, env, encoding: 'utf8', timeout: 60000 });
    const messages = JSON.parse(exported.stdout).messages.filter(m => m.type === 'user').map(m => m.text);
    step('opencode session continued', { userMessages: messages });
    check(messages.some(t => t.includes('What codeword did I ask you')), 'the panel turn is not in the original session');
  }
  const leftovers = fs.readdirSync(project);
  step('project untouched', { entries: leftovers });
  check(leftovers.length === 0, 'files were written into the project');
  evidence.passed = true;
  console.log(`[${agent}] PASS`);
} finally {
  await service.tryCall('serviceStop', {}, 10000);
  await sleep(500);
  try { process.kill(proc.pid); } catch {}
  fs.writeFileSync(path.join(work, 'evidence.json'), JSON.stringify(evidence, null, 1));
  console.log(`evidence: ${path.join(work, 'evidence.json')}`);
}
