// chrome-use teach recorder: watches what the user does in one page and
// reports it, for `chrome-use teach` (cli/src/native/teach.rs evaluates this
// file in a CDP isolated world) and the side panel's Record button (injected
// as a content script, also an isolated world). Never in the page's own world:
// the page cannot see its listeners, its state or the channel it reports on.
//
// It only reads. It changes nothing in the page, ignores events the page
// synthesised (isTrusted false) and never reports what was typed into a
// password, card or one-time-code field: those become `{secret: true}`.
//
// Installs as `globalThis.__cuTeachRecorder` in that isolated world:
//   install(emit)  start reporting; emit(event) receives plain JSON objects
//   uninstall()    remove every listener this file added
//   drain()        events not yet acknowledged (the CLI polls with this when
//                  it has no binding to receive them on)
(() => {
  const g = globalThis;
  if (g.__cuTeachRecorder) return;

  const norm = s => (s == null ? '' : String(s)).replace(/\s+/g, ' ').trim();
  const cut = (s, n = 80) => (s.length > n ? s.slice(0, n) : s);
  const esc = s => (g.CSS && CSS.escape ? CSS.escape(s) : String(s).replace(/[^\w-]/g, c => '\\' + c));

  const TEXT_TYPES = ['', 'text', 'email', 'tel', 'url', 'search', 'number', 'password', 'date', 'datetime-local', 'month', 'week', 'time'];
  const SECRET_AUTOCOMPLETE = /(?:^|\s)(?:current-password|new-password|one-time-code|cc-number|cc-csc|cc-exp|cc-exp-month|cc-exp-year)(?:\s|$)/i;
  const SECRET_NAME = /pass(?:word|wd|code)?|pwd|secret|token|otp|cvv|cvc|card.?num|security.?code/i;
  const INTERACTIVE = 'a[href],button,input,select,textarea,summary,label,[role=button],[role=link],[role=checkbox],[role=radio],[role=switch],[role=tab],[role=menuitem],[role=menuitemcheckbox],[role=menuitemradio],[role=option],[role=treeitem],[role=combobox],[contenteditable=""],[contenteditable=true],[onclick]';

  function isTextField(el) {
    if (!el || el.nodeType !== 1) return false;
    const tag = el.tagName;
    if (tag === 'TEXTAREA') return true;
    if (tag === 'INPUT') return TEXT_TYPES.includes((el.getAttribute('type') || '').toLowerCase());
    return !!el.isContentEditable;
  }

  function isSecret(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.tagName === 'INPUT' && (el.type || '').toLowerCase() === 'password') return true;
    if (SECRET_AUTOCOMPLETE.test(el.getAttribute('autocomplete') || '')) return true;
    return SECRET_NAME.test([el.name, el.id, el.getAttribute('aria-label')].filter(Boolean).join(' '));
  }

  function role(el) {
    const explicit = norm(el.getAttribute('role')).split(' ')[0];
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return el.hasAttribute('href') ? 'link' : '';
    if (tag === 'button' || tag === 'summary') return 'button';
    if (tag === 'select') return el.multiple || el.size > 1 ? 'listbox' : 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'input') {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      if (['button', 'submit', 'reset', 'image'].includes(t)) return 'button';
      if (t === 'checkbox') return 'checkbox';
      if (t === 'radio') return 'radio';
      if (t === 'range') return 'slider';
      if (t === 'number') return 'spinbutton';
      if (t === 'search') return el.hasAttribute('list') ? 'combobox' : 'searchbox';
      if (['text', 'email', 'tel', 'url', ''].includes(t)) return el.hasAttribute('list') ? 'combobox' : 'textbox';
      return '';
    }
    if (el.isContentEditable) return 'textbox';
    if (/^h[1-6]$/.test(tag)) return 'heading';
    return '';
  }

  // The label a person reads for a form control (aria-labelledby, aria-label,
  // <label>), the same sources Chrome uses first for its accessible name.
  function label(el) {
    const ids = norm(el.getAttribute('aria-labelledby'));
    if (ids) {
      const text = norm(ids.split(' ').map(id => document.getElementById(id)).filter(Boolean)
        .map(n => n.innerText || n.textContent).join(' '));
      if (text) return text;
    }
    const aria = norm(el.getAttribute('aria-label'));
    if (aria) return aria;
    if (el.labels && el.labels.length) {
      const text = norm(Array.from(el.labels).map(l => l.innerText || l.textContent).join(' '));
      if (text) return text;
    }
    return '';
  }

  function name(el, r) {
    const l = label(el);
    if (l) return l;
    const tag = el.tagName.toLowerCase();
    if (tag === 'input') {
      const t = (el.type || '').toLowerCase();
      if (['button', 'submit', 'reset'].includes(t)) return norm(el.value) || (t === 'reset' ? 'Reset' : t === 'submit' ? 'Submit' : '');
      if (t === 'image') return norm(el.alt);
    }
    if (['button', 'link', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'option', 'checkbox', 'radio', 'switch', 'treeitem', 'heading'].includes(r)) {
      const text = norm(el.innerText || el.textContent);
      if (text) return text;
      const img = el.querySelector && el.querySelector('img[alt]');
      if (img && norm(img.alt)) return norm(img.alt);
    }
    return norm(el.getAttribute('title')) || norm(el.getAttribute('placeholder'));
  }

  function unique(selector) {
    try { return document.querySelectorAll(selector).length === 1; } catch { return false; }
  }

  // An id or attribute that looks generated (React useId, long hex/digit runs)
  // is not one a replay a week later can count on.
  const generated = v => /^:|[0-9a-f]{8,}|\d{4,}|^(?:ember|react|mui|radix|headlessui)[-_:]/i.test(v);

  // A CSS selector that matches exactly this element now. `stable` says it
  // rests on an id, a test id or a name rather than on position.
  function css(el) {
    const tag = el.tagName.toLowerCase();
    if (el.id && !generated(el.id)) {
      const s = '#' + esc(el.id);
      if (unique(s)) return { css: s, stable: true };
    }
    for (const attr of ['data-testid', 'data-test', 'data-qa', 'data-cy']) {
      const v = el.getAttribute(attr);
      if (v) {
        const s = `[${attr}="${v.replace(/"/g, '\\"')}"]`;
        if (unique(s)) return { css: s, stable: true };
      }
    }
    if (el.name && /^(INPUT|SELECT|TEXTAREA|BUTTON)$/.test(el.tagName)) {
      let s = `${tag}[name="${String(el.name).replace(/"/g, '\\"')}"]`;
      if (unique(s)) return { css: s, stable: true };
      if (el.type === 'radio' || el.type === 'checkbox') {
        s += `[value="${String(el.value).replace(/"/g, '\\"')}"]`;
        if (unique(s)) return { css: s, stable: true };
      }
    }
    if (tag === 'a' && el.getAttribute('href')) {
      const s = `a[href="${el.getAttribute('href').replace(/"/g, '\\"')}"]`;
      if (el.getAttribute('href').length < 120 && unique(s)) return { css: s, stable: false };
    }
    // Structural path up to the nearest ancestor with a usable id.
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && node !== document.documentElement) {
      if (node !== el && node.id && !generated(node.id) && unique('#' + esc(node.id))) {
        parts.unshift('#' + esc(node.id));
        break;
      }
      const t = node.tagName.toLowerCase();
      const parent = node.parentElement;
      if (!parent) { parts.unshift(t); break; }
      const same = Array.from(parent.children).filter(c => c.tagName === node.tagName);
      parts.unshift(same.length > 1 ? `${t}:nth-of-type(${same.indexOf(node) + 1})` : t);
      node = parent;
    }
    const s = parts.join(' > ');
    return { css: unique(s) ? s : '', stable: false };
  }

  // How many elements a semantic locator would match, approximated in the
  // page (Chrome's accessible names can differ a little; replay uses those).
  function countRoleName(r, n) {
    let count = 0;
    for (const el of document.querySelectorAll(INTERACTIVE + ',h1,h2,h3,h4,h5,h6')) {
      if (role(el) === r && name(el, r).includes(n)) count++;
      if (count > 1) break;
    }
    return count;
  }

  function locate(el) {
    const r = role(el);
    const n = cut(name(el, r));
    const l = cut(label(el));
    const loc = {
      tag: el.tagName.toLowerCase(),
      type: el.tagName === 'INPUT' ? (el.type || 'text').toLowerCase() : undefined,
      role: r || undefined,
      name: n || undefined,
      label: l || undefined,
      placeholder: norm(el.getAttribute('placeholder')) || undefined,
      testid: el.getAttribute('data-testid') || undefined,
      ...css(el),
    };
    if (r && n) loc.nameUnique = countRoleName(r, n) === 1;
    return loc;
  }

  function target(e) {
    const path = typeof e.composedPath === 'function' ? e.composedPath() : [];
    const first = path.find(n => n && n.nodeType === 1) || e.target;
    return first && first.nodeType === 1 ? first : null;
  }

  let emitFn = null;
  let seq = 0;
  let pending = [];
  let listeners = [];
  let scrollState = null;
  // When the user last pressed a key or turned the wheel: a click the browser
  // makes from Enter (implicit form submission) is not a click of theirs, and
  // a scroll without wheel or key input is the page's own.
  let lastKeyAt = 0;
  let lastWheelAt = 0;

  function send(event) {
    event.seq = ++seq;
    event.ts = Date.now();
    event.url = location.href;
    pending.push(event);
    if (pending.length > 2000) pending.shift();
    flush();
  }

  // Hand over everything queued, oldest first; an emit that answers false
  // (no channel yet) leaves the rest queued for the next event or a drain().
  function flush() {
    if (!emitFn) return;
    while (pending.length) {
      try {
        if (emitFn(pending[0]) === false) return;
      } catch {
        return;
      }
      pending.shift();
    }
  }

  function onClick(e) {
    if (!e.isTrusted || e.button !== 0) return;
    if (e.detail === 0 && Date.now() - lastKeyAt < 500) return;
    const raw = target(e);
    if (!raw) return;
    const el = raw.closest(INTERACTIVE) || raw;
    // A checkbox, radio, select or file input reports its result on change.
    if (el.tagName === 'SELECT' || el.tagName === 'OPTION') return;
    if (el.tagName === 'INPUT' && ['checkbox', 'radio', 'file'].includes((el.type || '').toLowerCase())) return;
    // A label for a control: the control reports what changed.
    if (el.tagName === 'LABEL' && el.control) return;
    send({ t: 'click', loc: locate(el), text: isTextField(el) ? undefined : cut(norm(el.innerText || el.textContent), 60) || undefined, field: isTextField(el) || undefined });
  }

  function onInput(e) {
    if (!e.isTrusted) return;
    const el = target(e);
    if (!isTextField(el)) return;
    const field = el.isContentEditable && !('value' in el) ? (el.closest('[contenteditable]') || el) : el;
    if (isSecret(field)) send({ t: 'input', loc: locate(field), secret: true, field: field.name || undefined, hint: cut(norm(field.name || field.id || label(field) || 'secret'), 40) });
    else send({ t: 'input', loc: locate(field), value: field.isContentEditable && !('value' in field) ? field.innerText : field.value });
  }

  function onChange(e) {
    if (!e.isTrusted) return;
    const el = target(e);
    if (!el) return;
    if (el.tagName === 'SELECT') {
      const chosen = Array.from(el.selectedOptions);
      send({ t: 'select', loc: locate(el), values: chosen.map(o => o.value), labels: chosen.map(o => cut(norm(o.label || o.text), 60)) });
    } else if (el.tagName === 'INPUT' && ['checkbox', 'radio'].includes((el.type || '').toLowerCase())) {
      send({ t: 'check', loc: locate(el), checked: el.checked });
    } else if (el.tagName === 'INPUT' && (el.type || '').toLowerCase() === 'file') {
      send({ t: 'upload', loc: locate(el), files: Array.from(el.files || []).map(f => f.name), multiple: el.multiple || undefined });
    } else if (isTextField(el) && el.tagName === 'INPUT' && !['text', 'email', 'tel', 'url', 'search', 'password', ''].includes((el.type || '').toLowerCase())) {
      // Pickers (date, time, number spinners) can change a value without an input event.
      send({ t: 'input', loc: locate(el), value: el.value, secret: isSecret(el) || undefined });
    }
  }

  const NAV_KEYS = ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown', ' '];
  function onKeydown(e) {
    if (!e.isTrusted || e.isComposing) return;
    lastKeyAt = Date.now();
    const el = target(e);
    const text = isTextField(el);
    let key = null;
    if (e.key === 'Enter' && !(el && (el.tagName === 'TEXTAREA' || el.isContentEditable))) key = 'Enter';
    else if (e.key === 'Escape') key = 'Escape';
    else if (!text && NAV_KEYS.includes(e.key)) key = e.key === ' ' ? 'Space' : e.key;
    if (!key) return;
    send({ t: 'key', key, loc: el && el !== document.body ? locate(el) : undefined });
  }

  // A scroll is worth replaying only when it made the page load more (an
  // endless feed); otherwise the next action scrolls its own target into view.
  function onScroll(e) {
    if (!e.isTrusted) return;
    const now = Date.now();
    if (now - lastWheelAt > 1000 && now - lastKeyAt > 1000) return;
    const root = document.scrollingElement || document.documentElement;
    if (!scrollState) scrollState = { y0: root.scrollTop, h0: root.scrollHeight, href: location.href, timer: 0 };
    clearTimeout(scrollState.timer);
    scrollState.timer = setTimeout(() => {
      const s = scrollState;
      scrollState = null;
      setTimeout(() => {
        const grew = root.scrollHeight > s.h0 + 40 && location.href === s.href;
        const dy = Math.round(root.scrollTop - s.y0);
        if (grew && dy > 0) send({ t: 'scroll', dy });
      }, 700);
    }, 300);
  }

  function on(type, fn, target = g) {
    target.addEventListener(type, fn, true);
    listeners.push([target, type, fn]);
  }

  g.__cuTeachRecorder = {
    version: 1,
    install(emit) {
      this.uninstall();
      emitFn = typeof emit === 'function' ? emit : null;
      on('click', onClick);
      on('input', onInput);
      on('change', onChange);
      on('keydown', onKeydown);
      on('scroll', onScroll, document);
      on('wheel', e => { if (e.isTrusted) lastWheelAt = Date.now(); });
      return true;
    },
    uninstall() {
      for (const [t, type, fn] of listeners) t.removeEventListener(type, fn, true);
      listeners = [];
      emitFn = null;
      if (scrollState) clearTimeout(scrollState.timer);
      scrollState = null;
      return true;
    },
    drain() {
      flush();
      const out = pending;
      pending = [];
      return out;
    },
    active() {
      return listeners.length > 0;
    },
  };
})();
