// User-visible text lives in _locales/<lang>/messages.json. chrome.i18n picks
// the file for Chrome's UI language and falls back to English ("en"). When it
// has no text for a key (a stale message cache after an unpacked reload, a
// key Chrome has not picked up), the bundled English file answers instead, so
// a raw key never reaches the UI.
let fallback = null;

/** Expand a messages.json entry the way chrome.i18n.getMessage does: $NAME$
 *  placeholders to their content, then $1..$9 and $$. */
export function expandMessage(entry, substitutions = []) {
  const values = [].concat(substitutions ?? []).map(String);
  const placeholders = Object.fromEntries(Object.entries(entry.placeholders || {}).map(([name, value]) => [name.toLowerCase(), value.content]));
  const expanded = String(entry.message).replace(/\$([A-Za-z0-9_@]+)\$/g, (whole, name) => placeholders[name.toLowerCase()] ?? whole);
  return expanded.replace(/\$(\$|[1-9])/g, (whole, which) => which === '$' ? '$' : values[Number(which) - 1] ?? '');
}

/** Use `messages` (the parsed English messages.json) as the fallback. */
export function setFallbackMessages(messages) { fallback = messages && typeof messages === 'object' ? messages : null; }

function loadFallback() {
  try {
    const url = globalThis.chrome?.runtime?.getURL?.('_locales/en/messages.json');
    if (!url || typeof globalThis.fetch !== 'function') return Promise.resolve();
    return globalThis.fetch(url).then(response => response.json()).then(setFallbackMessages).catch(() => {});
  } catch { return Promise.resolve(); }
}
/** Resolves once the English fallback is loaded (or could not be). Pages
 *  await it before their first render. */
export const i18nReady = loadFallback();

function lookup(key, substitutions) {
  const i18n = globalThis.chrome?.i18n;
  const text = i18n?.getMessage && i18n.getMessage(key, substitutions.map(String));
  if (text) return text;
  const entry = fallback?.[key];
  return entry ? expandMessage(entry, substitutions) : '';
}

export function t(key, ...substitutions) {
  return lookup(key, substitutions) || key;
}

// A label table whose values are looked up when read, so modules can declare
// it at import time before chrome.i18n (or a test stub) is available.
export function messageMap(keys) {
  const map = {};
  for (const [name, key] of Object.entries(keys)) Object.defineProperty(map, name, { get: () => t(key), enumerable: true });
  return map;
}

const attributes = [['data-i18n', null], ['data-i18n-title', 'title'], ['data-i18n-placeholder', 'placeholder'], ['data-i18n-aria-label', 'aria-label']];

// Fill static page text from data-i18n* attributes: data-i18n sets the text
// content, the others set the named attribute. Without a message (a page
// opened outside the extension) the English written in the HTML stays.
export function applyI18n(root = globalThis.document) {
  const language = globalThis.chrome?.i18n?.getUILanguage?.();
  if (language && root?.documentElement) root.documentElement.lang = language;
  for (const [data, attribute] of attributes) {
    for (const element of root?.querySelectorAll?.(`[${data}]`) || []) {
      const text = lookup(element.getAttribute(data), []);
      if (!text) continue;
      if (attribute) element.setAttribute(attribute, text);
      else element.textContent = text;
    }
  }
}
