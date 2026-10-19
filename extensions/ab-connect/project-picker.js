// The project folder of a new conversation: the agent works there, while the
// panel's own files stay in the conversation's private folder. The choice is
// made once, at creation. A page of a repository starts in a recent checkout
// of it (the host matches remotes); a site can remember its project, for the
// whole site, a repository or a path, in the same store Inspect & Ask uses
// (one mapping, edited on the options page).
import { t } from './i18n.js';

export const PRIVATE = '';
const OTHER = '\u0000other';
// Hosts serving repositories at /owner/repo, for hosts too old to say.
const CODE_HOSTS = ['github.com', 'gitlab.com', 'gitee.com', 'bitbucket.org', 'codeberg.org'];

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

/** A scope as people read it: `github.com/owner/repo`. */
export function scopeName(origin, pathScope = '/') {
  let host = origin;
  try { host = new URL(origin).host; } catch {}
  return pathScope && pathScope !== '/' ? `${host}${pathScope}` : host;
}

/**
 * The scopes the remember bar offers for `site`, the default first:
 * `{ pathScope, kind: 'repository' | 'site' | 'path' }`. `info` is what the
 * host said about the page (`repositoryScope`, `codeHost`); `bound` the scope
 * of the site's current binding. A code host is never remembered whole by
 * default: its repository goes first, else a path; a page there with
 * neither is not asked about.
 */
export function rememberScopes(site, info, bound) {
  const segments = String(site.pagePath || '/').split('/').filter(Boolean);
  const prefix = n => `/${segments.slice(0, n).join('/')}`;
  let host = '';
  try { host = new URL(site.origin).hostname.replace(/^www\./, ''); } catch {}
  const known = CODE_HOSTS.includes(host);
  const repository = info ? info.repositoryScope : known && segments.length >= 2 ? prefix(2) : null;
  const codeHost = info ? Boolean(info.codeHost) : known;
  const scopes = [];
  const add = (pathScope, kind) => { if (pathScope && !scopes.some(s => s.pathScope === pathScope)) scopes.push({ pathScope, kind }); };
  const parent = segments.length > 1 ? prefix(segments.length - 1) : null;
  if (repository) {
    add(repository, 'repository');
    if (parent && parent.length > repository.length) add(parent, 'path');
    add('/', 'site');
  } else if (codeHost) {
    if (!segments.length) return [];
    if (bound && bound !== '/') add(bound, 'path');
    add(prefix(1), 'path');
    if (parent) add(parent, 'path');
    add('/', 'site');
  } else {
    add(bound || '/', bound && bound !== '/' ? 'path' : 'site');
    add('/', 'site');
    if (segments.length) add(prefix(1), 'path');
    if (parent) add(parent, 'path');
  }
  return scopes;
}

/**
 * `select`: the chip's <select>; `host`: where the folder-path entry, the
 * repository note and the "remember for this site" question appear.
 * `supported()` says whether the host can give conversations a project;
 * `canRemember()` whether it keeps site bindings; `matchesRepositories()`
 * whether it matches pages to checkouts (projectDirectory 2).
 */
export function createProjectChoice({ document, api, select, host, supported, canRemember = () => true, matchesRepositories = () => false, onError }) {
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
  // `suggestion`: the project a new conversation here starts in and why
  // (`repository` or `site`); `info`: what the host said about the page.
  let folders = [], extra = [], current = PRIVATE, chosenByUser = false, suggestion = null, info = null, bindings = [], site = null, agent = '', epoch = 0, previous = PRIVATE, quiet = false, askedOn = '';
  const manual = element('div', undefined, 'project-manual'); manual.hidden = true;
  const note = element('div', undefined, 'project-match hint'); note.hidden = true;
  const ask = element('div', undefined, 'project-remember'); ask.hidden = true;
  host?.append(manual, note, ask);

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
    select.title = pi ? t('panel_project_pi') : current ? current : t('panel_project_none_hint');
    // A chosen project reads as set; no project reads as an open slot.
    select.dataset.chosen = current && !pi ? '1' : '';
    const suggested = Boolean(suggestion && site && !pi && current === suggestion.root);
    const byRepository = suggested && suggestion.reason === 'repository';
    select.dataset.reason = suggested ? suggestion.reason : '';
    if (suggested) select.title = byRepository ? t('panel_project_repo_hint', scopeName(site.origin, suggestion.pathScope)) : t('panel_project_site_hint', scopeName(site.origin, suggestion.pathScope));
    note.textContent = byRepository ? t('panel_project_repo_hint', scopeName(site.origin, suggestion.pathScope)) : '';
    note.hidden = !byRepository || quiet;
  }
  function choose(path, byUser = true) { current = path; if (byUser) chosenByUser = true; if (path && !folders.some(f => f.path === path) && !extra.includes(path)) extra.unshift(path); render(); }

  /** What the host says about a page: suggestion, repository and bindings. */
  async function lookup(where) {
    if (!where || !canRemember()) return { list: await api('conversationProjectList').catch(() => ({ projects: [] })), page: null };
    if (matchesRepositories()) {
      const list = await api('conversationProjectList', where).catch(() => ({ projects: [] }));
      return { list, page: list.page || null };
    }
    // Hosts before projectDirectory 2 know only the site binding.
    const [list, found] = await Promise.all([
      api('conversationProjectList').catch(() => ({ projects: [] })),
      api('inspectProject', where).catch(() => ({ binding: null })),
    ]);
    const b = found.binding?.exists ? found.binding : null;
    return { list, page: { legacy: true, suggestion: b && { root: b.root, reason: 'site', pathScope: b.pathScope || '/' }, bindings: found.binding ? [found.binding] : [] } };
  }

  /** Load recent folders and the page's project. */
  async function refresh(page) {
    if (!supported()) { current = PRIVATE; return render(); }
    const mine = ++epoch;
    const where = siteOf(page?.url);
    const { list, page: about } = await lookup(where);
    if (mine !== epoch) return;
    site = where;
    // A question or its answer is about the page it was asked on.
    if (askedOn !== (where ? where.origin + where.pagePath : '')) ask.hidden = true;
    folders = list.projects || [];
    info = about?.legacy ? null : about;
    suggestion = about?.suggestion || null;
    bindings = about?.bindings || [];
    quiet = false;
    // The page's project is preselected until the user picks something.
    if (!chosenByUser) current = suggestion && agent !== 'pi' ? suggestion.root : PRIVATE;
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

  const SCOPE_LABELS = { repository: 'panel_project_scope_repository', site: 'panel_project_scope_site', path: 'panel_project_scope_path' };
  const scopeLabel = (origin, scope) => t(SCOPE_LABELS[scope.kind], scopeName(origin, scope.pathScope));
  /**
   * After a conversation was created in `project` on `page`: offer to
   * remember it, for the narrowest sensible scope, changeable in place.
   */
  async function created(project, page) {
    ask.hidden = true; quiet = true; note.hidden = true;
    const where = siteOf(page?.url);
    if (!project || !where || !canRemember()) return;
    // The page may have changed since the last refresh: ask the host again.
    let about = { info, suggestion, bindings };
    if (!site || where.origin !== site.origin || where.pagePath !== site.pagePath) {
      const fresh = (await lookup(where)).page;
      about = { info: fresh?.legacy ? null : fresh, suggestion: fresh?.suggestion || null, bindings: fresh?.bindings || [] };
    }
    if (about.suggestion?.root === project) return;
    const effective = about.bindings.filter(b => b.exists !== false).sort((a, b) => (b.pathScope || '/').length - (a.pathScope || '/').length)[0];
    const scopes = rememberScopes(where, about.info, effective?.pathScope);
    if (!scopes.length) return;
    let chosen = scopes[0];
    askedOn = where.origin + where.pagePath;
    const question = element('span', t('panel_project_remember', scopeName(where.origin, chosen.pathScope)));
    const parts = [question];
    if (scopes.length > 1) {
      const picker = element('select', undefined, 'project-scope');
      picker.setAttribute('aria-label', t('panel_project_scope_label'));
      picker.replaceChildren(...scopes.map(scope => option(scope.pathScope, scopeLabel(where.origin, scope))));
      picker.value = chosen.pathScope;
      picker.addEventListener('change', () => {
        chosen = scopes.find(scope => scope.pathScope === picker.value) || scopes[0];
        question.textContent = t('panel_project_remember', scopeName(where.origin, chosen.pathScope));
      });
      parts.push(picker);
    }
    ask.replaceChildren(...parts, button(t('panel_project_remember_yes'), async () => {
      const name = scopeName(where.origin, chosen.pathScope);
      const replaces = about.bindings.find(b => (b.pathScope || '/') === chosen.pathScope)?.fingerprint ?? null;
      const result = await api('inspectProjectBind', { origin: where.origin, pathScope: chosen.pathScope, root: project, replaces });
      const saved = { pathScope: chosen.pathScope, root: project, exists: true, fingerprint: result.binding?.fingerprint };
      bindings = [...about.bindings.filter(b => (b.pathScope || '/') !== chosen.pathScope), saved];
      if (site?.origin === where.origin) suggestion = { root: project, reason: 'site', pathScope: chosen.pathScope };
      ask.replaceChildren(element('span', t('panel_project_remembered', name)));
    }, 'primary'), button(t('panel_project_remember_no'), () => { ask.hidden = true; }));
    ask.hidden = false;
  }
  return {
    refresh, created,
    value: () => (agent === 'pi' || !supported() ? PRIVATE : current),
    setAgent(next) { agent = next; render(); },
    // After a creation: the next conversation starts from the page's project again.
    reset() { chosenByUser = false; extra = []; manual.hidden = true; },
    /** The project a new conversation on this page starts in, if any. */
    get preferred() { return suggestion?.root || ''; },
    get suggestion() { return suggestion; },
    hideQuestion() { ask.hidden = true; quiet = true; note.hidden = true; },
  };
}
