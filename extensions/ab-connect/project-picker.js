// The project folder of a new conversation: the agent works there, while the
// panel's own files stay in the conversation's private folder. The choice is
// made once, at creation. A site can remember its project in the same store
// Inspect & Ask uses (one mapping, edited on the options page).
import { t } from './i18n.js';

export const PRIVATE = '';
const OTHER = '\u0000other';

/** `name · parent` for a folder path. */
export function folderLabel(path) {
  const parts = String(path || '').split(/[\\/]+/).filter(Boolean);
  const name = parts.pop() || String(path || '');
  return parts.length ? `${name} · ${parts.slice(-2).join('/')}` : name;
}

/** The site a page belongs to, for Inspect & Ask bindings. */
export function siteOf(url) {
  try {
    const u = new URL(url);
    return /^https?:$/.test(u.protocol) ? { origin: u.origin, pagePath: u.pathname || '/' } : null;
  } catch { return null; }
}

/**
 * `select`: the chip's <select>; `host`: where the folder-path entry and the
 * "remember for this site" question appear. `supported()` says whether the
 * host can give conversations a project; `canRemember()` whether it keeps
 * site bindings.
 */
export function createProjectChoice({ document, api, select, host, supported, canRemember = () => true, onError }) {
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
  let folders = [], extra = [], current = PRIVATE, chosenByUser = false, binding = null, site = null, agent = '', epoch = 0, previous = PRIVATE;
  const manual = element('div', undefined, 'project-manual'); manual.hidden = true;
  const ask = element('div', undefined, 'project-remember'); ask.hidden = true;
  host?.append(manual, ask);

  function option(value, label, title) { const node = element('option', label); node.value = value; if (title) node.title = title; return node; }
  function render() {
    const paths = [...new Set([...extra, ...folders.map(f => f.path)])];
    if (current && !paths.includes(current)) paths.unshift(current);
    select.replaceChildren(
      option(PRIVATE, t('panel_project_private')),
      ...paths.map(path => option(path, folderLabel(path), path)),
      option(OTHER, t('panel_project_other')),
    );
    select.value = current;
    const pi = agent === 'pi';
    select.disabled = pi;
    select.title = pi ? t('panel_project_pi') : current ? current : t('panel_project_label');
    if (binding && current === binding.root) select.title = t('panel_project_site_hint', siteName());
  }
  function siteName() { return binding?.pathScope && binding.pathScope !== '/' ? `${binding.origin}${binding.pathScope}` : site?.origin || ''; }
  function choose(path, byUser = true) { current = path; if (byUser) chosenByUser = true; if (path && !folders.some(f => f.path === path) && !extra.includes(path)) extra.unshift(path); render(); }

  /** Load recent folders and the page's remembered project. */
  async function refresh(page) {
    if (!supported()) { current = PRIVATE; return render(); }
    const mine = ++epoch;
    site = siteOf(page?.url);
    const [list, found] = await Promise.all([
      api('conversationProjectList').catch(() => ({ projects: [] })),
      site && canRemember() ? api('inspectProject', site).catch(() => ({ binding: null })) : { binding: null },
    ]);
    if (mine !== epoch) return;
    folders = list.projects || [];
    binding = found.binding?.exists ? found.binding : null;
    // A remembered project is preselected until the user picks something.
    if (!chosenByUser) current = binding && agent !== 'pi' ? binding.root : PRIVATE;
    render();
  }
  async function other() {
    const result = await api('conversationProjectChoose', { prompt: t('panel_project_choose_prompt') });
    if (result.path) return choose(result.path);
    if (result.unavailable) return showManual();
    current = previous; render();
  }
  function showManual() {
    const input = element('input'); input.placeholder = t('panel_project_manual'); input.setAttribute('aria-label', t('panel_project_manual'));
    manual.replaceChildren(element('p', t('panel_project_manual_hint'), 'hint'), input, button(t('panel_project_use'), () => {
      const path = input.value.trim(); if (!path) return;
      manual.hidden = true; choose(path);
    }), button(t('panel_resume_cancel'), () => { manual.hidden = true; current = previous; render(); }));
    manual.hidden = false; input.focus?.();
  }
  select.addEventListener('change', () => {
    if (select.value === OTHER) { void other().catch(failure => { current = previous; render(); onError(failure); }); return; }
    previous = select.value; choose(select.value);
  });
  select.addEventListener('focus', () => { previous = current; });

  /** After a conversation was created in `project` on `page`: offer to remember it for the site. */
  function created(project, page) {
    ask.hidden = true;
    const where = siteOf(page?.url);
    if (!project || !where || !canRemember() || binding?.root === project) return;
    const scope = binding && where.origin === binding.origin ? binding : null;
    const name = scope?.pathScope && scope.pathScope !== '/' ? `${where.origin}${scope.pathScope}` : where.origin;
    ask.replaceChildren(element('span', t('panel_project_remember', name)), button(t('panel_project_remember_yes'), async () => {
      const result = await api('inspectProjectBind', { origin: where.origin, pathScope: scope?.pathScope || '/', root: project, replaces: scope?.fingerprint ?? null });
      binding = { ...result.binding, exists: true };
      ask.replaceChildren(element('span', t('panel_project_remembered', name)));
    }, 'primary'), button(t('panel_project_remember_no'), () => { ask.hidden = true; }));
    ask.hidden = false;
  }
  return {
    refresh, created,
    value: () => (agent === 'pi' || !supported() ? PRIVATE : current),
    setAgent(next) { agent = next; render(); },
    // After a creation: the next conversation starts from the site's project again.
    reset() { chosenByUser = false; extra = []; manual.hidden = true; },
    get binding() { return binding; },
    hideQuestion() { ask.hidden = true; },
  };
}
