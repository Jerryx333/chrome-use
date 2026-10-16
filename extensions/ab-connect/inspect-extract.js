// Inspect & Ask: element description for the picker content script.
// Injected as a classic script (chrome.scripting files) before inspect-picker.js
// and exposed on globalThis.__chromeUseInspectExtract inside the extension's
// isolated world; the page cannot see or replace it.
//
// Selector building, the secret-pattern filter, the computed-style subset and
// the text budgets are adapted from Orca (stablyai/orca, MIT, commit
// f31aada5b4f02e0a61d250716542518b9ce3cb83: src/main/browser/
// grab-guest-element-context-script.ts, grab-guest-content-script.ts,
// src/shared/browser-grab-types.ts). License text: THIRD_PARTY_NOTICES.md.
//
// Everything produced here is page data. It is clamped and labelled, never
// trusted to choose a project, a recipient or a permission.
(() => {
  const BUDGET = { text: 200, selected: 500, selector: 300, path: 200, classes: 200, html: 3000, nearby: 120, label: 80, attr: 300 };
  const SECRET_PATTERNS = ['access_token', 'auth_token', 'api_key', 'apikey', 'client_secret', 'oauth_state', 'x-amz-', 'session_id', 'sessionid', 'csrf', 'secret', 'password', 'passwd', 'bearer '];
  const STYLE_KEYS = [
    ['display', 'display'], ['position', 'position'], ['width', 'width'], ['height', 'height'],
    ['margin', 'margin'], ['padding', 'padding'], ['color', 'color'], ['background', 'backgroundColor'],
    ['border', 'border'], ['border-radius', 'borderRadius'], ['font-family', 'fontFamily'],
    ['font-size', 'fontSize'], ['font-weight', 'fontWeight'], ['line-height', 'lineHeight'],
    ['text-align', 'textAlign'], ['gap', 'gap'], ['flex-direction', 'flexDirection'], ['z-index', 'zIndex'],
  ];
  const DROP_ELEMENTS = 'script,style,noscript,template,link[rel="stylesheet"]';

  function clamp(value, max) {
    const text = String(value ?? '');
    return text.length > max ? text.slice(0, max) + '…' : text;
  }

  function inlineText(value, max) {
    let out = '';
    let space = false;
    const text = String(value ?? '');
    for (let i = 0; i < text.length && out.length < max; i++) {
      const ch = text[i];
      if (/\s/.test(ch)) { space = out.length > 0; continue; }
      if (space) { out += ' '; space = false; }
      out += ch;
    }
    return out.length >= max ? out.slice(0, max) + '…' : out;
  }

  function containsSecret(value) {
    const lower = String(value ?? '').toLowerCase();
    return SECRET_PATTERNS.some(p => lower.includes(p));
  }

  function sanitizeUrl(raw, base) {
    try {
      const url = new URL(String(raw), base);
      if (url.protocol === 'javascript:' || url.protocol === 'data:') return url.protocol;
      return url.origin === 'null' ? url.protocol : url.origin + url.pathname;
    } catch {
      return '';
    }
  }

  // code-inspector-plugin marks elements with data-insp-path="file:line:column:tag".
  // The file part may itself contain ':' (Windows drive letters).
  function parseInspPath(value) {
    if (typeof value !== 'string' || !value || value.length > 1000) return null;
    const parts = value.split(':');
    if (parts.length < 3) return null;
    let tag = '';
    if (!/^\d+$/.test(parts[parts.length - 1])) tag = parts.pop();
    const column = parts.pop();
    const line = parts.pop();
    if (!/^\d+$/.test(line || '') || !/^\d+$/.test(column || '')) return null;
    const path = parts.join(':');
    if (!path || containsSecret(path)) return null;
    return { kind: 'insp', path: clamp(path, 300), line: Number(line), column: Number(column), tag: clamp(tag, 40) };
  }

  function cssEscape(value) {
    return globalThis.CSS?.escape ? CSS.escape(value) : String(value).replace(/[^a-zA-Z0-9_-]/g, ch => '\\' + ch);
  }

  function looksHashy(cls) {
    return /^[a-z]{1,4}-[a-z0-9]{5,}$/i.test(cls) || /[0-9a-f]{6,}/i.test(cls) || /^_[a-zA-Z0-9]{5,}$/.test(cls);
  }

  function stableClasses(el, limit) {
    const out = [];
    for (const cls of el.classList || []) {
      if (out.length >= limit) break;
      if (!cls || cls.length > 60 || containsSecret(cls) || /^css-[a-z0-9]+$/i.test(cls) || looksHashy(cls)) continue;
      out.push(cls);
    }
    return out;
  }

  function selectorPart(el) {
    const tag = el.tagName.toLowerCase();
    if (el.id && !containsSecret(el.id) && el.id.length < 80) return tag + '#' + cssEscape(el.id);
    const classes = stableClasses(el, 2);
    return classes.length ? tag + classes.map(c => '.' + cssEscape(c)).join('') : tag;
  }

  function unique(scope, selector) {
    try { return scope.querySelectorAll(selector).length === 1; } catch { return false; }
  }

  function nthOfType(el) {
    let index = 1;
    for (let s = el.previousElementSibling; s; s = s.previousElementSibling) if (s.tagName === el.tagName) index++;
    if (index > 1) return ':nth-of-type(' + index + ')';
    for (let s = el.nextElementSibling; s; s = s.nextElementSibling) if (s.tagName === el.tagName) return ':nth-of-type(1)';
    return '';
  }

  // Unique within the element's own tree: the document, or the shadow root
  // it lives in (a selector never crosses a shadow boundary).
  function buildSelector(el) {
    const doc = el.ownerDocument;
    const scope = el.getRootNode?.() || doc;
    const parts = [];
    let current = el;
    while (current && current.nodeType === 1 && current !== doc.body && current !== doc.documentElement && parts.length < 10) {
      let part = selectorPart(current);
      if (current.parentElement && !unique(scope, [part, ...parts].join(' > '))) part += nthOfType(current);
      parts.unshift(part);
      const selector = parts.join(' > ');
      if (unique(scope, selector)) return clamp(selector, BUDGET.selector);
      current = current.parentElement;
    }
    return clamp(parts.join(' > ') || el.tagName.toLowerCase(), BUDGET.selector);
  }

  // Shadow hosts between the element and its document, outermost first.
  function shadowHosts(el) {
    const hosts = [];
    for (let root = el.getRootNode?.(); root && root.host && hosts.length < 8; root = root.host.getRootNode?.()) hosts.unshift(buildSelector(root.host));
    return hosts;
  }

  // Frames between the element's document and the top document, outermost
  // first: the <iframe> selector in its parent and the frame's own address.
  function frameChain(el) {
    const frames = [];
    for (let win = el.ownerDocument.defaultView; win && win !== win.top && frames.length < 4; win = win.parent) {
      let owner = null;
      try { owner = win.frameElement; } catch {}
      if (!owner) break;
      frames.unshift({ selector: buildSelector(owner), url: sanitizeUrl(win.location.href), hosts: shadowHosts(owner) });
    }
    return frames;
  }

  // A cross-origin <iframe> picked as a whole: what it shows comes from
  // another site the picker cannot read.
  function foreignFrame(el) {
    if (el.tagName !== 'IFRAME' && el.tagName !== 'FRAME') return null;
    try { if (el.contentDocument) return null; } catch {}
    return { url: sanitizeUrl(el.getAttribute('src') || '', el.ownerDocument.baseURI) };
  }

  // Bounds in the top window's viewport: frames add their offsets.
  function topRect(el) {
    const r = el.getBoundingClientRect();
    let x = r.left, y = r.top;
    for (let win = el.ownerDocument.defaultView; win && win !== win.top; win = win.parent) {
      let owner = null;
      try { owner = win.frameElement; } catch {}
      if (!owner) break;
      const o = owner.getBoundingClientRect();
      x += o.left + owner.clientLeft;
      y += o.top + owner.clientTop;
    }
    return { left: x, top: y, width: r.width, height: r.height, right: x + r.width, bottom: y + r.height };
  }

  function readablePath(el) {
    const parts = [];
    for (let current = el; current && parts.length < 6; current = current.parentElement) {
      const tag = current.tagName.toLowerCase();
      if (tag === 'html' || tag === 'body') break;
      const aria = current.getAttribute('aria-label');
      if (current.id && !containsSecret(current.id)) parts.unshift('#' + cssEscape(current.id));
      else if (aria && !containsSecret(aria)) parts.unshift(tag + '[aria-label="' + inlineText(aria, 40).replace(/"/g, '\\"') + '"]');
      else {
        const cls = stableClasses(current, 1);
        parts.unshift(cls.length ? tag + '.' + cssEscape(cls[0]) : tag);
      }
    }
    return clamp(parts.join(' > '), BUDGET.path);
  }

  function accessibleName(el) {
    const aria = el.getAttribute('aria-label');
    if (aria) return aria;
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {
      const names = labelledBy.split(/\s+/).slice(0, 8).map(id => el.ownerDocument.getElementById(id)?.textContent || '').filter(Boolean);
      if (names.length) return names.join(' ');
    }
    if (el.tagName === 'IMG') return el.getAttribute('alt') || '';
    if (el.tagName === 'INPUT') return el.getAttribute('placeholder') || el.getAttribute('name') || '';
    return el.getAttribute('title') || '';
  }

  function textOf(el) {
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') return '';
    return inlineText(el.innerText ?? el.textContent ?? '', BUDGET.text);
  }

  function selectedTextIn(el) {
    try {
      const selection = el.ownerDocument.getSelection();
      if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return '';
      const range = selection.getRangeAt(0);
      if (!el.contains(range.commonAncestorContainer) && !range.intersectsNode(el)) return '';
      return inlineText(selection.toString(), BUDGET.selected);
    } catch {
      return '';
    }
  }

  function nearbyText(el) {
    const out = [];
    const push = node => {
      if (!node || out.length >= 3) return;
      const text = inlineText(node.innerText ?? node.textContent ?? '', BUDGET.nearby);
      if (text && !containsSecret(text)) out.push(text);
    };
    push(el.previousElementSibling);
    push(el.nextElementSibling);
    const heading = el.closest('section,article,form,dialog,[role="dialog"]')?.querySelector('h1,h2,h3,h4,legend');
    if (heading && heading !== el) push(heading);
    return out;
  }

  function computedStyles(el) {
    const view = el.ownerDocument.defaultView;
    const style = view.getComputedStyle(el);
    const lines = [];
    for (const [name, key] of STYLE_KEYS) {
      const value = style[key];
      if (!value || value === 'auto' || value === 'normal' || value === 'none') continue;
      if (name === 'position' && value === 'static') continue;
      if (name === 'background' && value === 'rgba(0, 0, 0, 0)') continue;
      if (name === 'gap' && value === 'normal') continue;
      lines.push([name, clamp(value, 120)]);
    }
    return lines;
  }

  // Strip anything executable or secret before the HTML leaves the page.
  function htmlSnippet(el) {
    const clone = el.cloneNode(true);
    for (const node of clone.querySelectorAll(DROP_ELEMENTS)) node.remove();
    const all = [clone, ...clone.querySelectorAll('*')];
    for (const node of all) {
      for (const attr of [...node.attributes]) {
        const name = attr.name.toLowerCase();
        if (name.startsWith('on') || name === 'srcdoc' || name === 'style' && attr.value.length > 300) { node.removeAttribute(attr.name); continue; }
        if (name === 'value' || (node.tagName === 'INPUT' && name === 'checked')) { node.setAttribute(attr.name, ''); continue; }
        if (name === 'href' || name === 'src' || name === 'action' || name === 'formaction' || name === 'poster') {
          node.setAttribute(attr.name, sanitizeUrl(attr.value, el.ownerDocument.baseURI));
          continue;
        }
        if (containsSecret(name) || containsSecret(attr.value)) { node.setAttribute(attr.name, '[redacted]'); continue; }
        if (attr.value.length > BUDGET.attr) node.setAttribute(attr.name, attr.value.slice(0, BUDGET.attr) + '…');
      }
      if (node.tagName === 'TEXTAREA') node.textContent = '';
    }
    return clamp(clone.outerHTML || '', BUDGET.html);
  }

  function sourceMark(el) {
    for (let current = el, depth = 0; current && current.nodeType === 1 && depth < 12; current = current.parentElement, depth++) {
      const parsed = parseInspPath(current.getAttribute('data-insp-path'));
      if (parsed) return { ...parsed, inherited: current !== el };
    }
    return null;
  }

  function describe(el) {
    const rect = topRect(el);
    const name = accessibleName(el);
    const text = textOf(el);
    const tag = el.tagName.toLowerCase();
    const labelText = !containsSecret(name) && name ? name : text;
    return {
      tagName: tag,
      label: clamp(labelText ? `${tag} "${inlineText(labelText, 60)}"` : tag, BUDGET.label),
      selector: buildSelector(el),
      path: readablePath(el),
      classes: containsSecret(el.getAttribute('class')) ? '[redacted]' : clamp(el.getAttribute('class') || '', BUDGET.classes),
      rect: { x: Math.round(rect.left), y: Math.round(rect.top), width: Math.round(rect.width), height: Math.round(rect.height) },
      text: containsSecret(text) ? '[redacted]' : text,
      selectedText: selectedTextIn(el),
      nearby: nearbyText(el),
      styles: computedStyles(el),
      html: htmlSnippet(el),
      source: sourceMark(el),
      shadowHosts: shadowHosts(el),
      frames: frameChain(el),
      foreignFrame: foreignFrame(el),
    };
  }

  function layout(win) {
    return {
      innerWidth: win.innerWidth, innerHeight: win.innerHeight,
      scrollX: Math.round(win.scrollX), scrollY: Math.round(win.scrollY),
      devicePixelRatio: win.devicePixelRatio || 1,
    };
  }

  globalThis.__chromeUseInspectExtract = Object.freeze({
    BUDGET, clamp, inlineText, containsSecret, sanitizeUrl, parseInspPath, looksHashy,
    buildSelector, readablePath, describe, sourceMark, htmlSnippet, layout, topRect, shadowHosts, frameChain,
  });
})();
