// Options page for chrome-use.
// Settings live in chrome.storage.sync; the service worker reads them to decide
// whether to fire connection notifications and whether to draw the agent cursor.
// Opens standalone (file://) too, with a friendly demo state, so the design is
// viewable without the extension context.

const DEFAULTS = { ab_notify: false, ab_cursor: false, ab_idle_detach_secs: 0 }
const hasChrome = typeof chrome !== 'undefined' && chrome.storage && chrome.storage.sync

// ---- elements ----
const el = (id) => document.getElementById(id)
const dot = el('dot'), statusLabel = el('statusLabel'), statusSub = el('statusSub'), tabPill = el('tabPill')
const optNotify = el('optNotify'), optCursor = el('optCursor'), optIdleSecs = el('optIdleSecs')
const saved = el('saved')

// ---- settings ----
function loadSettings(cb) {
  if (!hasChrome) { cb({ ...DEFAULTS, ab_notify: true, ab_cursor: true }); return }
  chrome.storage.sync.get(DEFAULTS, (s) => cb(s))
}
function save(patch) {
  if (hasChrome) chrome.storage.sync.set(patch)
  flashSaved()
}
let savedTimer
function flashSaved() {
  saved.classList.add('show')
  clearTimeout(savedTimer)
  savedTimer = setTimeout(() => saved.classList.remove('show'), 1400)
}


// ---- connection status ----
function renderStatus(state) {
  const connected = !!(state && state.connected)
  dot.classList.remove('on', 'off')
  if (connected) {
    dot.classList.add('on')
    statusLabel.textContent = 'Connected'
    statusSub.textContent = 'bridged to your local CLI · ready'
    const n = state.tabCount | 0
    if (n > 0) { tabPill.textContent = `${n} tab${n === 1 ? '' : 's'} attached`; tabPill.style.display = '' }
    else tabPill.style.display = 'none'
  } else {
    dot.classList.add('off')
    statusLabel.textContent = 'Not paired'
    statusSub.textContent = 'run `chrome-use extension install`, then reopen'
    tabPill.style.display = 'none'
  }
}
function queryStatus() {
  if (!(hasChrome && chrome.runtime && chrome.runtime.sendMessage)) { renderStatus({ connected: true, tabCount: 13 }); return }
  try {
    chrome.runtime.sendMessage({ type: 'ab-status' }, (resp) => {
      renderStatus(chrome.runtime.lastError ? { connected: false } : resp)
    })
  } catch { renderStatus({ connected: false }) }
}

// ---- wire up ----
el('ver').textContent = hasChrome ? 'v' + chrome.runtime.getManifest().version : 'v—'

loadSettings((s) => {
  optNotify.checked = !!s.ab_notify
  optCursor.checked = !!s.ab_cursor
  optIdleSecs.value = Number.isFinite(Number(s.ab_idle_detach_secs)) ? Number(s.ab_idle_detach_secs) : 0
})

optNotify.addEventListener('change', () => save({ ab_notify: optNotify.checked }))
optCursor.addEventListener('change', () => save({ ab_cursor: optCursor.checked }))
optIdleSecs.addEventListener('change', () => {
  const n = Math.max(0, Math.min(3600, Number(optIdleSecs.value) || 0))
  optIdleSecs.value = n
  save({ ab_idle_detach_secs: n })
})

el('copyCmd').addEventListener('click', () => {
  const cmd = el('silentCmd').textContent
  navigator.clipboard?.writeText(cmd).then(() => {
    const b = el('copyCmd'); const t = b.textContent; b.textContent = 'Copied ✓'
    setTimeout(() => (b.textContent = t), 1300)
  }).catch(() => {})
})

// Open external links (docs / repo / site) in a real tab. Every [data-href]
// anchor is wired the same way.
document.querySelectorAll('[data-href]').forEach((a) => {
  a.addEventListener('click', (e) => {
    e.preventDefault()
    const url = a.dataset.href
    if (hasChrome && chrome.tabs) chrome.tabs.create({ url })
    else window.open(url, '_blank')
  })
})

queryStatus()
setTimeout(queryStatus, 700)

// ---- Inspect & Ask ----
const INSPECT_KEY = 'inspectSettings'
const optInspect = el('optInspect'), optInspectEditor = el('optInspectEditor')
function loadInspect() {
  if (!hasChrome) return
  chrome.storage.local.get(INSPECT_KEY, (stored) => {
    const settings = stored?.[INSPECT_KEY] || {}
    optInspect.checked = settings.enabled === true
    optInspectEditor.value = settings.editor || 'vscode'
  })
  chrome.commands?.getAll?.((commands) => {
    const command = (commands || []).find((c) => c.name === 'inspect-element')
    if (command) el('inspectShortcut').textContent = command.shortcut || 'not set'
  })
}
function saveInspect(patch) {
  if (!hasChrome) return
  chrome.storage.local.get(INSPECT_KEY, (stored) => {
    chrome.storage.local.set({ [INSPECT_KEY]: { ...(stored?.[INSPECT_KEY] || {}), ...patch } })
    flashSaved()
  })
}
optInspect?.addEventListener('change', () => saveInspect({ enabled: optInspect.checked }))
optInspectEditor?.addEventListener('change', () => saveInspect({ editor: optInspectEditor.value }))
el('inspectShortcuts')?.addEventListener('click', (event) => {
  event.preventDefault()
  if (hasChrome) chrome.tabs.create({ url: 'chrome://extensions/shortcuts' })
})
function inspectRequest(operation, params = {}) {
  return new Promise((resolve, reject) => {
    chrome.runtime.sendMessage({ type: 'ab-agent-request', operation, params }, (response) => {
      if (chrome.runtime.lastError) return reject(new Error(chrome.runtime.lastError.message))
      if (!response?.success) return reject(new Error(response?.error || 'Request failed'))
      resolve(response.result)
    })
  })
}
async function renderBindings() {
  const hint = el('inspectBindingsHint'), list = el('inspectBindings')
  if (!hasChrome) { hint.textContent = 'Bindings appear here once a site is bound.'; return }
  try {
    const { bindings } = await inspectRequest('inspectBindings')
    list.replaceChildren()
    hint.textContent = bindings.length ? 'Remove a binding to choose again next time. Consents are kept per project.' : 'No site is bound yet. The first Send on a site asks which project it belongs to.'
    for (const binding of bindings) {
      const row = document.createElement('div')
      row.className = 'row'
      const text = document.createElement('div')
      text.className = 'grow'
      const name = document.createElement('div'); name.className = 'name'; name.textContent = binding.origin
      const detail = document.createElement('div'); detail.className = 'hint'
      const consents = Object.entries(binding.consents || {}).filter(([, v]) => v).map(([k]) => k === 'plugin' ? 'dev source mapping' : 'test-build source mapping')
      detail.textContent = `${binding.root}${binding.sessionLabel ? ' · ' + binding.sessionLabel : ''}${consents.length ? ' · consented: ' + consents.join(', ') : ''}`
      text.append(name, detail)
      row.append(text)
      for (const kind of ['plugin', 'testEnv']) {
        if (!binding.consents?.[kind]) continue
        const revoke = document.createElement('button'); revoke.className = 'btn'; revoke.textContent = kind === 'plugin' ? 'Revoke dev consent' : 'Revoke test consent'
        revoke.addEventListener('click', async () => { await inspectRequest('inspectConsent', { origin: binding.origin, kind, granted: false }).catch(() => {}); renderBindings() })
        row.append(revoke)
      }
      const remove = document.createElement('button'); remove.className = 'btn'; remove.textContent = 'Unbind'
      remove.addEventListener('click', async () => { await inspectRequest('inspectUnbind', { origin: binding.origin }).catch(() => {}); renderBindings() })
      row.append(remove)
      list.append(row)
    }
  } catch (error) {
    hint.textContent = `Bindings unavailable: ${error.message}`
  }
}
loadInspect()
renderBindings()
