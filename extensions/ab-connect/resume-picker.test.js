import './test-i18n.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import { parseResumeCommand, projectName, importedView, createResumePicker, RESUME_TEXT } from './resume-picker.js';

class Node {
  constructor(tag = 'div') { this.tag = tag; this.children = []; this.listeners = {}; this.textContent = ''; this.hidden = false; this.value = ''; this.attributes = {}; }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  addEventListener(type, listener) { this.listeners[type] = listener; }
  setAttribute(name, value) { this.attributes[name] = value; }
  fire(type, details = {}) { this.listeners[type]?.({ preventDefault() {}, ...details }); }
}
const document = { createElement: tag => new Node(tag) };
const text = node => [node.textContent, ...node.children.map(text)].filter(Boolean).join(' ');
const all = (node, predicate) => [...(predicate(node) ? [node] : []), ...node.children.flatMap(child => all(child, predicate))];
const settle = async () => { for (let i = 0; i < 4; i++) await new Promise(resolve => setImmediate(resolve)); };

test('the command is /resume with an optional search', () => {
  assert.deepEqual(parseResumeCommand('/resume'), { query: '' });
  assert.deepEqual(parseResumeCommand('  /resume   fix login  '), { query: 'fix login' });
  for (const other of ['/resumes', '/resume-x', 'resume', 'please /resume', '/review']) assert.equal(parseResumeCommand(other), null, other);
  assert.equal(projectName('/Users/me/work/app/'), 'app');
  assert.equal(projectName('C:\\work\\app'), 'app');
  assert.equal(projectName(null), '');
});

test('imported history says what was not recorded', () => {
  assert.equal(importedView(document, null), null);
  const complete = importedView(document, { messages: [{ role: 'user', text: 'hi' }], end: { agentId: 'pi', cwd: '/p', omitted: 0, unparsed: 0, partial: false } }, { agentName: () => 'Pi' });
  assert.doesNotMatch(text(complete), /not recorded/);
  assert.match(text(complete), /Continued from a Pi session in \/p\. Earlier in this session · 1 message You hi/);
  const partial = importedView(document, { messages: [], end: { agentId: 'pi', partial: true, omitted: 0 } });
  assert.match(text(partial), new RegExp(RESUME_TEXT.notRecorded.replace('.', '\\.')));
  // No closing event: the import itself did not finish.
  assert.match(text(importedView(document, { messages: [{ role: 'tool', text: 'Read' }], end: null })), /not recorded/);
});

test('the picker lists, searches and opens an already continued session without resuming it again', async () => {
  const calls = [], opened = [];
  let pending = null;
  const picker = createResumePicker({
    document,
    api: async (operation, params) => { calls.push([operation, params]); return { schemaVersion: 1, agentId: params.agentId, supported: params.agentId !== 'pi', sessions: [{ sessionId: 's1', agentId: params.agentId, title: null, cwd: '/w/app', lastActive: 0, running: false, conversationId: 'conv-1' }] }; },
    agents: () => [{ id: 'codex', name: 'Codex' }, { id: 'pi', name: 'Pi' }],
    preferredAgent: () => 'codex',
    executable: agentId => agentId === 'codex' ? '/opt/codex' : '',
    onResumed: async (conversation, existing) => opened.push([conversation.id, existing]),
    onError: error => { throw error; },
    schedule: callback => { pending = callback; return 1; },
    cancel: () => { pending = null; },
  });
  await picker.open('app');
  assert.equal(picker.isOpen, true);
  assert.deepEqual(calls[0], ['conversationSessionList', { agentId: 'codex', limit: 50, query: 'app', executable: '/opt/codex' }]);
  assert.match(text(picker.container), /Untitled session Codex · app/);
  assert.match(text(picker.container), /Open in panel/);
  // Typing searches after a pause.
  const search = all(picker.container, n => n.tag === 'input')[0];
  search.value = 'other'; search.fire('input'); assert.ok(pending); pending(); await settle();
  assert.equal(calls[1][1].query, 'other');
  // Choosing a continued session opens it; nothing is resumed twice.
  all(picker.container, n => n.className?.split?.(' ').includes('resume-item'))[0].fire('click'); await settle();
  assert.deepEqual(opened, [['conv-1', true]]);
  assert.ok(!calls.some(([operation]) => operation === 'conversationResume'));
  assert.equal(picker.isOpen, false);
  // An agent whose sessions cannot be continued says so.
  await picker.open('', 'pi');
  assert.match(text(picker.container), /Pi sessions can't be continued here/);
});
