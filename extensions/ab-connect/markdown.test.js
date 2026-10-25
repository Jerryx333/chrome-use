import assert from 'node:assert/strict';
import test from 'node:test';
import { renderMarkdown } from './markdown.js';

class Node {
  constructor(tag) { this.tag = tag; this.children = []; this.attributes = {}; this.text = ''; this.className = ''; }
  set textContent(value) { this.children = []; this.text = value; }
  get textContent() { return this.text + this.children.map(child => child.textContent).join(''); }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren() { this.children = []; this.text = ''; }
  setAttribute(name, value) { this.attributes[name] = value; }
}
const document = { createElement: tag => new Node(tag), createTextNode: text => Object.assign(new Node('#text'), { text }) };
const render = source => { const root = new Node('div'); renderMarkdown(document, root, source); return root; };
const shape = node => node.tag === '#text' ? node.text : { [node.tag]: node.children.length ? node.children.map(shape) : node.text };

test('headings, emphasis, inline code and lists become elements', () => {
  const root = render('## 私有仓库 (3个)\n\n1. **api-gateway** - TypeScript\n2. `codex-review` 的配置\n\n- 📚 文档\n- 🔐 KYC');
  assert.deepEqual(root.children.map(n => n.tag), ['h4', 'ol', 'ul']);
  assert.deepEqual(shape(root.children[1].children[0]), { li: [{ strong: ['api-gateway'] }, ' - TypeScript'] });
  assert.deepEqual(shape(root.children[1].children[1]), { li: [{ code: 'codex-review' }, ' 的配置'] });
  assert.equal(root.children[2].children.length, 2);
});

test('identifiers with underscores are not italicised', () => {
  const root = render('Called mcp__chrome_use__chrome_use_open and _done_.');
  assert.deepEqual(shape(root.children[0]), { p: ['Called mcp__chrome_use__chrome_use_open and ', { em: ['done'] }, '.'] });
});

test('fenced code keeps its text literally, including markup', () => {
  const root = render('```html\n<script>alert(1)</script>\n**not bold**\n```');
  assert.equal(root.children[0].tag, 'pre');
  assert.equal(root.children[0].attributes['data-language'], 'html');
  assert.equal(root.children[0].children[0].text, '<script>alert(1)</script>\n**not bold**');
});

test('long fences preserve shorter nested examples and the language label', () => {
  for (const marker of ['`', '~']) {
    const body = `${marker.repeat(3)}js\nconst answer = 42;\n${marker.repeat(3)}`;
    const root = render(`${marker.repeat(4)}markdown\n${body}\n${marker.repeat(4)}\n\nAfter`);
    assert.deepEqual(root.children.map(node => node.tag), ['pre', 'p']);
    assert.equal(root.children[0].attributes['data-language'], 'markdown');
    assert.equal(root.children[0].children[0].textContent, body);
    assert.equal(root.children[1].textContent, 'After');
  }
});

test('fence-like lines with trailing text remain literal code', () => {
  for (const marker of ['```', '~~~']) {
    const body = `before\n${marker}not-a-closing-fence\n${marker} trailing text\n<script>alert(1)</script>\nafter`;
    const root = render(`${marker}text\n${body}\n${marker}`);
    assert.deepEqual(root.children.map(node => node.tag), ['pre']);
    assert.equal(root.children[0].children[0].textContent, body);
    assert.equal(root.children[0].children[0].children.length, 0);
  }
});

test('closing fences must use the opening character and at least its length', () => {
  for (const [marker, other] of [['`', '~'], ['~', '`']]) {
    const body = `before\n${other.repeat(5)}\n${marker.repeat(3)}\nafter`;
    const root = render(`${marker.repeat(4)}\n${body}\n  ${marker.repeat(5)} \t\nAfter`);
    assert.deepEqual(root.children.map(node => node.tag), ['pre', 'p']);
    assert.equal(root.children[0].children[0].textContent, body);
    assert.equal(root.children[0].attributes['data-language'], undefined);
    assert.equal(root.children[1].textContent, 'After');
  }
});

test('an unfinished streamed code block keeps all remaining text', () => {
  for (const marker of ['````', '~~~~']) {
    const body = `first\n\n${marker.slice(1)}\nlast`;
    const root = render(`Before\n${marker}text\n${body}`);
    assert.deepEqual(root.children.map(node => node.tag), ['p', 'pre']);
    assert.equal(root.children[1].children[0].textContent, body);
  }
});

test('only http(s) links become anchors, and they open safely', () => {
  const root = render('[ok](https://example.com/a) [bad](javascript:alert(1)) https://x.com/p.');
  const anchors = root.children[0].children.filter(n => n.tag === 'a');
  assert.equal(anchors.length, 2);
  assert.equal(anchors[0].attributes.href, 'https://example.com/a');
  assert.equal(anchors[0].attributes.rel, 'noopener noreferrer');
  assert.equal(anchors[1].attributes.href, 'https://x.com/p');
  assert.match(root.textContent, /\[bad\]\(javascript:alert\(1\)\)/);
});

test('pipe tables, quotes and line breaks inside a paragraph', () => {
  const root = render('| a | b |\n|---|---|\n| 1 | **2** |\n\n> quoted\n\nline one\nline two');
  assert.equal(root.children[0].children[0].tag, 'table');
  assert.equal(root.children[1].tag, 'blockquote');
  assert.deepEqual(shape(root.children[2]), { p: ['line one', { br: '' }, 'line two'] });
});
