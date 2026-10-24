// Cursor state belongs to the extension worker; cleanup leaves no replacement
// marker in the page. Injected expressions run only on existing attachments.

/** Remove the overlay (including its shadow tree) and verify both artifacts. */
export const REMOVE_CURSOR_EXPRESSION = `(() => {
  const c = window.__abCursor;
  if (!c) return !Object.prototype.hasOwnProperty.call(window, '__abCursor');
  clearTimeout(c.t);
  for (const id of c.timers || []) clearTimeout(id);
  for (const id of c.frames || []) cancelAnimationFrame(id);
  if (c.host) c.host.remove();
  delete window.__abCursor;
  return (!c.host || !c.host.isConnected) &&
    !Object.prototype.hasOwnProperty.call(window, '__abCursor');
})()`;

/**
 * Serialize cursor work per tab so a late draw cannot overtake its cleanup.
 * evaluate must use an existing debugger connection, without attach/recovery.
 * A failed cleanup stays pending for an explicit retry or normal reattachment.
 */
export function createCursorOverlay({ evaluate, warn = console.warn }) {
  let enabled = false;
  let generation = 0;
  const records = new Map();

  function record(tabId) {
    if (!records.has(tabId)) records.set(tabId, { tail: Promise.resolve(), pending: false });
    return records.get(tabId);
  }

  function enqueue(tabId, state, operation) {
    const next = state.tail.then(() => records.get(tabId) === state ? operation() : false);
    state.tail = next.catch(() => {});
    return next;
  }

  async function confirmed(tabId, expression) {
    const reply = await evaluate(tabId, expression);
    return !reply?.exceptionDetails && reply?.result?.value === true;
  }

  function remove(tabId) {
    const state = record(tabId);
    state.pending = true;
    return enqueue(tabId, state, async () => {
      try {
        if (await confirmed(tabId, REMOVE_CURSOR_EXPRESSION)) {
          state.pending = false;
          return true;
        }
      } catch {}
      warn(`Cursor cleanup unconfirmed for tab ${tabId}; reload the page before relying on stealth.`);
      return false;
    });
  }

  function update(tabId, expression) {
    if (!enabled) return Promise.resolve(false);
    const state = record(tabId);
    const version = generation;
    return enqueue(tabId, state, async () => {
      if (!enabled || version !== generation) return false;
      try { return await confirmed(tabId, expression); } catch { return false; }
    });
  }

  return {
    setEnabled(value, attachedTabs = []) {
      if (enabled !== value) generation++;
      enabled = value;
      if (enabled) return Promise.resolve([]);
      // Include failed/queued draws: an error does not prove no overlay exists.
      const candidates = new Set([...attachedTabs, ...records.keys()]);
      return Promise.all([...candidates].map(remove));
    },
    draw(tabId, x, y, click) {
      if (!Number.isFinite(x) || !Number.isFinite(y)) return Promise.resolve(false);
      return update(tabId, cursorOverlayExpression(x, y, click));
    },
    visible(tabId, show) {
      return update(tabId, `(() => {
        const c = window.__abCursor;
        if (!c?.host?.isConnected) return false;
        c.host.style.display = ${JSON.stringify(show ? '' : 'none')};
        return c.host.style.display === ${JSON.stringify(show ? '' : 'none')};
      })()`);
    },
    attached(tabId) {
      // Restored, idle-detached tabs still own their page DOM.
      const state = record(tabId);
      return enabled && !state.pending ? Promise.resolve(true) : remove(tabId);
    },
    // Only a closed/replaced tab is forgotten; detachment does not erase its DOM.
    forget(tabId) { records.delete(tabId); },
    pendingTabs() { return [...records].filter(([, state]) => state.pending).map(([tabId]) => tabId); },
  };
}

/** Draw only after the worker has authorized non-stealth cursor use. */
export function cursorOverlayExpression(x, y, click) {
  return (
    '(function(){try{' +
    'var X=' +
    x +
    ',Y=' +
    y +
    ',CLICK=' +
    (click ? 'true' : 'false') +
    ';' +
    'var d=document,w=window,c=w.__abCursor;' +
    'if(!c||!c.host||!c.host.isConnected){' +
    'var host=d.createElement("div");host.style.cssText="position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none";' +
    '(d.documentElement||d.body).appendChild(host);' +
    'var sr=host.attachShadow?host.attachShadow({mode:"open"}):host;' +
    'var glow=d.createElement("div");glow.style.cssText="position:fixed;left:0;top:0;width:34px;height:34px;margin:-17px 0 0 -17px;border-radius:50%;pointer-events:none;opacity:0;background:radial-gradient(circle,rgba(247,168,35,.55),rgba(247,168,35,0) 70%);transition:transform .13s cubic-bezier(.2,.7,.3,1),opacity .3s";' +
    'var cur=d.createElement("div");cur.style.cssText="position:fixed;left:0;top:0;width:24px;height:24px;margin:-2px 0 0 -4px;pointer-events:none;opacity:0;transition:transform .13s cubic-bezier(.2,.7,.3,1),opacity .3s";' +
    'cur.innerHTML=\'<svg width="24" height="24" viewBox="0 0 24 24" fill="none"><path d="M4 2 L4 19 L9 14.5 L12 21 L14.6 19.8 L11.6 13.4 L18 13 Z" fill="#fff" stroke="#1bb6c9" stroke-width="1.6" stroke-linejoin="round"/></svg>\';' +
    // A new cursor starts where it is drawn: placed at (X,Y) with its
    // transition off, so it does not slide in from the corner of the page.
    'cur.style.transform=glow.style.transform="translate("+X+"px,"+Y+"px)";var tr=cur.style.transition,gt=glow.style.transition;cur.style.transition=glow.style.transition="opacity .3s";' +
    'sr.appendChild(glow);sr.appendChild(cur);cur.getBoundingClientRect();cur.style.transition=tr;glow.style.transition=gt;c=w.__abCursor={host:host,sr:sr,cur:cur,glow:glow,t:0};' +
    '}' +
    'c.timers=c.timers||new Set();c.frames=c.frames||new Set();' +
    'c.host.style.display="";var el=c.cur,gl=c.glow;el.style.opacity="1";gl.style.opacity="1";' +
    'el.style.transform="translate("+X+"px,"+Y+"px)";gl.style.transform="translate("+X+"px,"+Y+"px)";' +
    'if(CLICK){var r=d.createElement("div");r.style.cssText="position:fixed;left:"+X+"px;top:"+Y+"px;width:10px;height:10px;margin:-5px 0 0 -5px;border:2px solid #f7a823;border-radius:50%;pointer-events:none;opacity:.9;transition:transform .5s ease-out,opacity .5s ease-out";c.sr.appendChild(r);var frame=requestAnimationFrame(function(){c.frames.delete(frame);r.style.transform="scale(4)";r.style.opacity="0"});c.frames.add(frame);var timer=setTimeout(function(){c.timers.delete(timer);r.remove()},520);c.timers.add(timer);}' +
    'clearTimeout(c.t);c.t=setTimeout(function(){el.style.opacity="0";gl.style.opacity="0"},2200);' +
    'return !!c.host.isConnected;}catch(e){return false;}})()'
  );
}
