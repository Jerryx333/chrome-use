// Inspect & Ask: framework probe, run with chrome.scripting.executeScript
// ({world: 'MAIN'}) because React fibers and Vue component instances are
// expando properties visible only in the page's own JavaScript world.
//
// The probe reads elements the picker tagged with a random attribute name and
// returns component names and, for dev builds, a source file. Its output is
// page-controlled (the page can patch anything in the MAIN world), so
// validateProbeResult clamps it and callers treat it as a hint only.
//
// React fiber walking and the source-path cleaning are adapted from Orca
// (stablyai/orca, MIT, commit f31aada5b4f02e0a61d250716542518b9ce3cb83:
// src/main/browser/grab-guest-react-script.ts). License: THIRD_PARTY_NOTICES.md.

// Must stay self-contained: executeScript serializes the function source.
export function inspectProbe(attributeName) {
  const SKIP = /^(Fragment|Root|Routes|Route|Outlet|Provider|Consumer|Profiler|Suspense|StrictMode)$|(?:Boundary|BoundaryHandler|Router|Provider|Consumer|Context|Wrapper)$|^(Inner|Outer|Client|Server|RSC|Dev|React|Hot)/;
  function clean(path) {
    return String(path || '')
      .replace(/[?#].*$/, '')
      .replace(/^turbopack:\/\/\/\[project\]\//, '')
      .replace(/^webpack-internal:\/\/\/\.\//, '')
      .replace(/^webpack-internal:\/\/\//, '')
      .replace(/^webpack:\/\/\/\.\//, '')
      .replace(/^webpack:\/\/\//, '')
      .replace(/^turbopack:\/\/\//, '')
      .replace(/^https?:\/\/[^/]+\//, '')
      .replace(/^file:\/\/\//, '/')
      .replace(/^\([^)]+\)\/\.\//, '')
      .replace(/^\.\//, '');
  }
  function fiberOf(el) {
    for (const key of Object.keys(el)) {
      if (key.startsWith('__reactFiber$') || key.startsWith('__reactInternalInstance$')) {
        try { return el[key] || null; } catch { return null; }
      }
    }
    return null;
  }
  function nameOf(type) {
    if (!type || typeof type === 'string') return null;
    return type.displayName || type.name || type.render?.displayName || type.render?.name || type.type?.displayName || type.type?.name || null;
  }
  function react(el) {
    let fiber = fiberOf(el);
    if (!fiber) return null;
    const names = [];
    let source = null;
    for (let depth = 0; fiber && depth < 40; depth++, fiber = fiber.return) {
      const name = nameOf(fiber.type || fiber.elementType);
      if (name && name.length > 2 && !SKIP.test(name) && !names.includes(name) && names.length < 6) names.push(name);
      const debug = fiber._debugSource || fiber._debugOwner?._debugSource;
      if (!source && debug?.fileName && debug.lineNumber) {
        source = { kind: 'react', path: clean(debug.fileName), line: debug.lineNumber, column: debug.columnNumber ?? null };
      }
    }
    return { framework: 'react', component: names[0] || null, ancestors: names.slice(1, 4), source };
  }
  function vue(el) {
    for (let node = el, depth = 0; node && depth < 12; node = node.parentElement, depth++) {
      const instance = node.__vueParentComponent || null;
      if (instance) {
        const names = [];
        let source = null;
        for (let current = instance, d = 0; current && d < 12; current = current.parent, d++) {
          const type = current.type || {};
          const name = type.name || type.__name || (type.__file ? type.__file.split('/').pop().replace(/\.vue$/, '') : null);
          if (name && !names.includes(name) && names.length < 6) names.push(name);
          if (!source && type.__file) source = { kind: 'vue', path: clean(type.__file), line: null, column: null };
        }
        return { framework: 'vue', component: names[0] || null, ancestors: names.slice(1, 4), source };
      }
      const legacy = node.__vue__;
      if (legacy) {
        const file = legacy.$options?.__file;
        return { framework: 'vue', component: legacy.$options?.name || null, ancestors: [], source: file ? { kind: 'vue', path: clean(file), line: null, column: null } : null };
      }
    }
    return null;
  }
  const out = [];
  for (const el of document.querySelectorAll('[' + attributeName + ']')) {
    let info = null;
    try { info = react(el) || vue(el); } catch {}
    out.push({ index: Number(el.getAttribute(attributeName)), info });
  }
  return out;
}

const PATH_MAX = 300;
const NAME_MAX = 80;

function cleanName(value) {
  return typeof value === 'string' && value && value.length <= NAME_MAX && /^[\w$.:-]+$/.test(value) ? value : null;
}

function cleanSource(source) {
  if (!source || typeof source !== 'object') return null;
  if (!['react', 'vue'].includes(source.kind) || typeof source.path !== 'string') return null;
  const path = source.path.trim();
  if (!path || path.length > PATH_MAX || /[\u0000-\u001f]/.test(path) || /(secret|token|password|passwd|api_key|apikey)/i.test(path)) return null;
  const line = Number.isInteger(source.line) && source.line > 0 && source.line < 1e7 ? source.line : null;
  const column = Number.isInteger(source.column) && source.column >= 0 && source.column < 1e5 ? source.column : null;
  return { kind: source.kind, path, line, column };
}

// Map probe output back to picker indexes, dropping anything malformed.
export function validateProbeResult(raw, count) {
  const result = Array.from({ length: count }, () => null);
  if (!Array.isArray(raw)) return result;
  for (const entry of raw.slice(0, 64)) {
    const index = entry?.index;
    if (!Number.isInteger(index) || index < 0 || index >= count || !entry.info || typeof entry.info !== 'object') continue;
    const info = entry.info;
    result[index] = {
      framework: info.framework === 'react' || info.framework === 'vue' ? info.framework : null,
      component: cleanName(info.component),
      ancestors: Array.isArray(info.ancestors) ? info.ancestors.map(cleanName).filter(Boolean).slice(0, 3) : [],
      source: cleanSource(info.source),
    };
  }
  return result;
}
