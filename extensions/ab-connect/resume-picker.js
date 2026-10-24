// `/resume`: continue an agent CLI session that was started outside the panel
// (a terminal, an IDE). The picker lists the agent's recent sessions on this
// computer; choosing one asks the host to bind a new panel conversation to it.
// Nothing is sent to the agent until the user writes the next message.

import { t, messageMap } from './i18n.js';
import { folderLabel } from './project-picker.js';

// The feature's text: plain labels are looked up when read (chrome.i18n may
// not exist yet at import time), the rest take their values.
export const RESUME_TEXT = Object.defineProperties({
  // The command word itself is not translated.
  command: 'resume',
  emptyQuery: query => t('panel_resume_empty_query', query),
  unsupported: agent => t('panel_resume_unsupported', agent),
  earlier: count => count === 1 ? t('panel_resume_earlier_one') : t('panel_resume_earlier', count),
  omitted: count => count === 1 ? t('panel_resume_omitted_one') : t('panel_resume_omitted', count),
  continued: (agent, cwd) => cwd ? t('panel_resume_continued_in', agent, cwd) : t('panel_resume_continued', agent),
  showMore: count => t('panel_resume_show_more', count),
  projectToggle: folder => t('panel_resume_project_toggle', folder),
}, Object.getOwnPropertyDescriptors(messageMap({
  commandHint: 'panel_resume_command_hint', menuItem: 'panel_resume_menu_item', title: 'panel_resume_title',
  close: 'panel_resume_close', agent: 'panel_resume_agent', search: 'panel_resume_search', loading: 'panel_resume_loading',
  empty: 'panel_resume_empty', untitled: 'panel_resume_untitled', running: 'panel_resume_running', open: 'panel_resume_open',
  resuming: 'panel_resume_resuming', updateCli: 'panel_resume_update_cli', unsafe: 'panel_resume_unsafe',
  unsafeHint: 'panel_resume_unsafe_hint', confirm: 'panel_resume_confirm', cancel: 'panel_resume_cancel',
  ownership: 'panel_resume_ownership', deleteKeeps: 'panel_resume_delete_keeps', notRecorded: 'panel_resume_not_recorded',
  you: 'panel_speaker_you', assistant: 'panel_speaker_assistant', tools: 'panel_resume_tools',
  other: 'panel_resume_other', showLess: 'panel_resume_show_less', siteProject: 'panel_resume_site_project',
})));

/** Sessions a project shows before "Show more". */
export const PROJECT_PREVIEW = 3;

/** A folder path compared without its trailing separators. */
function folderKey(path) {
  const text = String(path || '');
  return text.replace(/[\\/]+$/, '') || text;
}

/**
 * Sessions grouped by their folder, like Codex's project list: the project
 * with the latest session first, sessions without a folder last ("Other").
 * `preferred` (the current site's project) goes to the top when it has
 * sessions. Each group keeps the listing's order.
 */
export function groupSessions(rows, preferred = '') {
  const groups = new Map();
  for (const row of rows) {
    const key = row.cwd ? folderKey(row.cwd) : '';
    let group = groups.get(key);
    if (!group) { group = { key, path: key, sessions: [], latest: 0 }; groups.set(key, group); }
    group.sessions.push(row);
    group.latest = Math.max(group.latest, Number(row.lastActive) || 0);
  }
  const site = preferred ? folderKey(preferred) : '';
  const rank = group => !group.key ? 2 : group.key === site ? 0 : 1;
  return [...groups.values()].sort((a, b) => rank(a) - rank(b) || b.latest - a.latest);
}

/** `/resume [query]` typed in the composer, or null for any other text. */
export function parseResumeCommand(text) {
  const match = /^\/resume(?:\s+([\s\S]*))?$/.exec(String(text || '').trim());
  return match ? { query: (match[1] || '').trim() } : null;
}

/** The last folder name of a session's directory. */
export function projectName(cwd) {
  const parts = String(cwd || '').split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] || '';
}

/**
 * The picker. `agents()` lists `{id, name}` of the panel's chat agents,
 * `preferredAgent()` the one to show first, `executable(agentId)` an
 * optional custom CLI path, and `onResumed(conversation, existing)` opens the
 * resumed (or already continued) conversation.
 */
export function createResumePicker({ document, api, agents, preferredAgent, preferredProject = () => '', executable = () => '', onResumed, onError, relativeTime = () => '', schedule = setTimeout, cancel = clearTimeout }) {
  const element = (tag, text, className) => {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  const button = (text, action, className = 'quiet') => {
    const node = element('button', text, className); node.type = 'button';
    node.addEventListener('click', () => void Promise.resolve().then(action).catch(onError));
    return node;
  };
  const container = element('section', undefined, 'resume-picker'); container.hidden = true;
  container.setAttribute('role', 'dialog'); container.setAttribute('aria-label', RESUME_TEXT.title);
  const head = element('div', undefined, 'resume-head');
  const agent = element('select'); agent.setAttribute('aria-label', RESUME_TEXT.agent);
  head.append(element('strong', RESUME_TEXT.title), agent, button(RESUME_TEXT.close, () => close()));
  const search = element('input'); search.type = 'search'; search.placeholder = RESUME_TEXT.search; search.setAttribute('aria-label', RESUME_TEXT.search);
  const status = element('p', '', 'hint');
  const items = element('div', undefined, 'resume-items'); items.setAttribute('role', 'listbox'); items.setAttribute('aria-label', RESUME_TEXT.title);
  const confirm = element('section', undefined, 'delete-confirm'); confirm.hidden = true; confirm.setAttribute('role', 'alertdialog');
  container.append(head, search, status, items, confirm, element('p', RESUME_TEXT.ownership, 'hint'));
  let epoch = 0, timer = null, busy = false, rows = [];
  // Per project, kept while the picker lives: collapsed headers and
  // projects showing all their sessions. `options` are the session rows on
  // screen, in order, for the arrow keys; `active` indexes them.
  const collapsed = new Set(), expanded = new Set();
  let options = [], active = -1;

  /** Clear visible and keyboard choices together, and invalidate older listings. */
  function resetResults() {
    items.replaceChildren(); rows = []; options = []; active = -1;
    return ++epoch;
  }
  function close() { container.hidden = true; confirm.hidden = true; resetResults(); if (timer !== null) cancel(timer); timer = null; }
  function open(query = '', agentId = '') {
    const list = agents();
    agent.replaceChildren(...list.map(({ id, name }) => { const option = element('option', name); option.value = id; return option; }));
    const wanted = agentId || preferredAgent();
    agent.value = list.some(a => a.id === wanted) ? wanted : list[0]?.id || '';
    search.value = query; confirm.hidden = true; container.hidden = false;
    search.focus?.();
    return load();
  }
  function nameOf(id) { return agents().find(a => a.id === id)?.name || id; }
  async function load() {
    if (timer !== null) cancel(timer); timer = null;
    const current = resetResults(), agentId = agent.value, query = search.value.trim();
    status.textContent = RESUME_TEXT.loading; status.hidden = false;
    const params = { agentId, limit: 100 };
    if (query) params.query = query;
    const path = executable(agentId); if (path) params.executable = path;
    let result;
    try { result = await api('conversationSessionList', params); }
    catch (failure) { if (current === epoch) status.textContent = String(failure?.message || failure); return; }
    if (current !== epoch) return;
    if (!result.supported) { status.textContent = RESUME_TEXT.unsupported(nameOf(agentId)); return; }
    rows = result.sessions;
    status.textContent = rows.length ? '' : query ? RESUME_TEXT.emptyQuery(query) : RESUME_TEXT.empty;
    status.hidden = Boolean(rows.length);
    active = -1;
    render();
  }
  function sessionItem(row) {
    const item = element('button', undefined, 'conversation-item resume-item'); item.type = 'button'; item.setAttribute('role', 'option');
    item.setAttribute('aria-selected', 'false');
    if (row.cwd) item.title = row.cwd;
    const title = element('span', row.title || RESUME_TEXT.untitled, 'title');
    const meta = element('span', relativeTime(row.lastActive), 'meta');
    item.append(title, meta);
    if (row.running) item.append(element('span', RESUME_TEXT.running, 'badge running'));
    if (row.conversationId) item.append(element('span', RESUME_TEXT.open, 'badge'));
    item.addEventListener('click', () => void choose(row).catch(onError));
    return item;
  }
  // A search shows every match: nothing is collapsed or cut short.
  function render() {
    const searching = Boolean(search.value.trim()), site = preferredProject() || '';
    const nodes = []; options = [];
    const groups = groupSessions(rows, site);
    // A folder's name, with its parents when another project shares it.
    const names = groups.map(g => projectName(g.path));
    for (const group of groups) {
      const short = projectName(group.path);
      const name = !group.key ? RESUME_TEXT.other : names.filter(n => n === short).length > 1 ? folderLabel(group.path) : short;
      const closed = !searching && collapsed.has(group.key);
      const header = element('button', undefined, 'resume-project'); header.type = 'button';
      header.setAttribute('aria-expanded', String(!closed));
      header.setAttribute('aria-label', RESUME_TEXT.projectToggle(name));
      if (group.key) header.title = group.path;
      header.append(element('span', closed ? '▸' : '▾', 'caret'), element('span', name, 'name'));
      if (group.key && group.key === folderKey(site)) header.append(element('span', RESUME_TEXT.siteProject, 'badge'));
      header.append(element('span', String(group.sessions.length), 'count'));
      header.addEventListener('click', () => { if (collapsed.has(group.key)) collapsed.delete(group.key); else collapsed.add(group.key); render(); });
      nodes.push(header);
      if (closed) continue;
      const all = searching || expanded.has(group.key);
      const shown = all ? group.sessions : group.sessions.slice(0, PROJECT_PREVIEW);
      for (const row of shown) { const item = sessionItem(row); options.push({ item, row }); nodes.push(item); }
      if (!searching && group.sessions.length > PROJECT_PREVIEW) {
        const more = button(all ? RESUME_TEXT.showLess : RESUME_TEXT.showMore(group.sessions.length - PROJECT_PREVIEW), () => {
          if (all) expanded.delete(group.key); else expanded.add(group.key);
          render();
        }, 'quiet resume-more');
        nodes.push(more);
      }
    }
    items.replaceChildren(...nodes);
    highlight(Math.min(active, options.length - 1));
  }
  function highlight(index) {
    active = index;
    options.forEach(({ item }, i) => {
      const on = i === active;
      item.setAttribute('aria-selected', String(on));
      item.className = `conversation-item resume-item${on ? ' selected' : ''}`;
      if (on) item.scrollIntoView?.({ block: 'nearest' });
    });
  }
  function move(step) {
    if (!options.length) return;
    highlight(active < 0 ? (step > 0 ? 0 : options.length - 1) : (active + step + options.length) % options.length);
  }
  async function choose(row, confirmRunning = false) {
    if (busy) return;
    if (row.conversationId) { close(); await onResumed({ id: row.conversationId }, true); return; }
    busy = true; status.textContent = RESUME_TEXT.resuming; status.hidden = false;
    try {
      const params = { agentId: row.agentId, sessionId: row.sessionId };
      if (confirmRunning) params.confirmRunning = true;
      const path = executable(row.agentId); if (path) params.executable = path;
      const result = await api('conversationResume', params);
      if (result.confirmRequired === 'running') { askRunning(row); return; }
      close();
      await onResumed(result.conversation, result.existing === true, result.projectRefused || null);
    } finally { busy = false; if (!container.hidden && status.textContent === RESUME_TEXT.resuming) { status.textContent = ''; status.hidden = true; } }
  }
  // A session another terminal may still be writing is never attached
  // silently: the user confirms, and the host checks again.
  function askRunning(row) {
    confirm.replaceChildren(element('p', RESUME_TEXT.unsafe), element('p', RESUME_TEXT.unsafeHint, 'hint'));
    const actions = element('div', undefined, 'actions');
    actions.append(button(RESUME_TEXT.confirm, () => { confirm.hidden = true; return choose(row, true); }, 'danger'), button(RESUME_TEXT.cancel, () => { confirm.hidden = true; }));
    confirm.append(actions); confirm.hidden = false;
  }
  agent.addEventListener('change', () => void load());
  search.addEventListener('input', () => {
    // Invalidate immediately: an earlier request may finish before debounce fires.
    resetResults(); status.textContent = RESUME_TEXT.loading; status.hidden = false;
    if (timer !== null) cancel(timer);
    timer = schedule(() => { timer = null; void load(); }, 250);
  });
  // The search box keeps the focus: arrows move through the sessions on
  // screen, Enter continues the highlighted one (or the first).
  search.addEventListener('keydown', event => {
    if (event.key === 'Escape') { event.preventDefault?.(); close(); }
    else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault?.(); move(event.key === 'ArrowDown' ? 1 : -1); }
    else if (event.key === 'Enter' && !event.isComposing && confirm.hidden && options.length) {
      event.preventDefault?.();
      void choose(options[Math.max(active, 0)].row).catch(onError);
    }
  });
  return { container, open, close, choose, get rows() { return rows; }, get isOpen() { return !container.hidden; } };
}

/**
 * The collapsed block of history imported from the CLI session, or null.
 * `imported` is the timeline's projection of `importedHistory` events.
 */
export function importedView(document, imported, { agentName = id => id, renderMarkdown } = {}) {
  if (!imported?.messages?.length && !imported?.end) return null;
  const element = (tag, text, className) => {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  const end = imported.end || {};
  const block = element('section', undefined, 'imported-history');
  block.append(element('p', RESUME_TEXT.continued(agentName(end.agentId), end.cwd || ''), 'hint'));
  const details = element('details');
  details.append(element('summary', RESUME_TEXT.earlier(imported.messages.length)));
  if (end.partial || end.omitted > 0 || end.unparsed > 0 || !imported.end) {
    const note = element('p', RESUME_TEXT.notRecorded, 'hint');
    if (end.omitted > 0) note.textContent += ` ${RESUME_TEXT.omitted(end.omitted)}`;
    details.append(note);
  }
  for (const message of imported.messages) {
    const row = element('div', undefined, `imported-message ${message.role}`);
    row.append(element('div', message.role === 'user' ? RESUME_TEXT.you : message.role === 'tool' ? RESUME_TEXT.tools : RESUME_TEXT.assistant, 'speaker'));
    const content = element('div', undefined, 'content');
    if (message.role === 'assistant' && renderMarkdown) renderMarkdown(document, content, message.text);
    else content.textContent = message.text;
    row.append(content); details.append(row);
  }
  block.append(details);
  return block;
}
