// First-run setup from the extension alone (#781 G4). A user who added the
// extension from the Chrome Web Store has no local CLI yet, so the panel has
// nothing to talk to. The popup and side panel then show a guided setup:
//
// - macOS: one button downloads chrome-use-darwin-<arch>.dmg from the latest
//   GitHub Release (signed, notarized, stapled). Opening ChromeUse.app in it
//   installs the CLI for this user and registers the native host
//   (cli/src/gui_setup.rs). No terminal, no admin password.
// - Windows: the PowerShell line, with a copy button (no signed installer).
// - Linux: the install.sh line, with a copy button.
//
// While the guide shows, the page asks the worker to retry the host every
// few seconds (`ab-host-retry`, which skips the reconnect backoff), so the
// panel continues by itself once the installer finishes. Chrome reads the
// host manifest on every connect; nothing needs a reload.
//
// After the host answers, the panel checks the agent CLIs it can chat with
// (`discover` + `agentInspect`, the same reads as Agents & models) and names
// the next step for each: ready, sign in (the command), or install (a link).
//
// Everything here lives in extension pages and the worker. Nothing reaches a
// web page, so stealth mode is unaffected.
//
// The top half is DOM-free and unit-tested (onboarding.test.js); the render
// functions take the document and chrome objects as arguments.

import { missingHostHint } from './native-host-hint.js'
import { INSTALL_COMMAND, WINDOWS_UPGRADE_COMMAND } from './version-notice.js'

export const RELEASE_DOWNLOAD = 'https://github.com/leeguooooo/chrome-use/releases/latest/download'
export const INSTALL_DOCS = 'https://chrome-use.leeguoo.com/install.html'
/** How often a page showing the setup guide asks the worker to retry. */
export const HOST_POLL_MS = 2500

/** 'mac' | 'windows' | 'linux' from navigator's platform string. */
export function detectOs(platform) {
  const p = String(platform || '')
  if (/mac|darwin/i.test(p)) return 'mac'
  if (/win/i.test(p)) return 'windows'
  return 'linux'
}

export function macDownloadUrl(arch) {
  return `${RELEASE_DOWNLOAD}/chrome-use-darwin-${arch === 'x64' ? 'x64' : 'arm64'}.dmg`
}

/**
 * What the setup guide offers on this machine. `architecture` is
 * userAgentData's high-entropy value ('arm' | 'x86'), or '' when unknown,
 * in which case both Mac downloads are offered.
 */
export function hostSetupPlan({ platform = '', architecture = '' } = {}) {
  const os = detectOs(platform)
  if (os === 'mac') {
    const arches = architecture === 'arm' ? ['arm64'] : architecture === 'x86' ? ['x64'] : ['arm64', 'x64']
    return {
      os,
      downloads: arches.map(arch => ({ arch, url: macDownloadUrl(arch), labelKey: arches.length > 1 ? (arch === 'arm64' ? 'panel_onboard_download_mac_arm' : 'panel_onboard_download_mac_intel') : 'panel_onboard_download_mac' })),
      command: INSTALL_COMMAND,
      steps: ['panel_onboard_mac_step_download', 'panel_onboard_mac_step_open', 'panel_onboard_step_back'],
    }
  }
  if (os === 'windows') {
    return { os, downloads: [], command: WINDOWS_UPGRADE_COMMAND, steps: ['panel_onboard_win_step_open', 'panel_onboard_win_step_paste', 'panel_onboard_step_back'] }
  }
  return { os, downloads: [], command: INSTALL_COMMAND, steps: ['panel_onboard_linux_step_open', 'panel_onboard_linux_step_paste', 'panel_onboard_step_back'] }
}

/** Whether the status the worker reports means "no CLI on this machine". */
export function needsHostSetup(state) {
  if (!state || state.standalone || state.connected) return false
  if (state.connectionState === 'connecting') return false
  return missingHostHint(state.connectionError) !== null
}

/**
 * Keeps the guide up through the brief 'connecting' states each retry
 * produces, so it does not flicker, until the host is confirmed.
 */
export class HostSetupTracker {
  constructor() { this.active = false; this.completed = false }
  update(state) {
    if (state?.connected) {
      if (this.active) this.completed = true
      this.active = false
    } else if (needsHostSetup(state)) {
      this.active = true
    } else if (state?.connectionState !== 'connecting') {
      // A host that exists but fails differently has its own message.
      this.active = false
    }
    return this.active
  }
}

/** The agents the onboarding names, in the order it lists them. */
export const AGENT_SETUP = {
  codex: { name: 'Codex', signIn: 'codex login', installUrl: 'https://developers.openai.com/codex/cli' },
  claude: { name: 'Claude Code', signIn: 'claude', installUrl: 'https://docs.claude.com/en/docs/claude-code/setup' },
  opencode: { name: 'OpenCode', signIn: 'opencode auth login', installUrl: 'https://opencode.ai/docs/' },
  pi: { name: 'Pi', signIn: 'pi', installUrl: 'https://github.com/badlogic/pi-mono' },
}

/**
 * The next step for one agent: `discovered` is its `discover` entry (or
 * undefined), `inspection` its `agentInspect` result, `failed` whether that
 * read failed. States: 'ready' | 'signIn' | 'unverified' | 'install'.
 */
export function agentNextStep(id, discovered, inspection, failed = false) {
  const setup = AGENT_SETUP[id]
  if (!setup) return null
  const base = { id, name: setup.name }
  if (!discovered?.executable) return { ...base, state: 'install', url: setup.installUrl }
  const status = inspection?.account?.status
  if (status === 'reportedSignedIn') return { ...base, state: 'ready', kind: typeof inspection.account.kind === 'string' ? inspection.account.kind : null }
  if (status === 'reportedSignedOut') return { ...base, state: 'signIn', command: setup.signIn }
  // Not read (no "Check sign-in" yet), unreadable, or not reported.
  return { ...base, state: 'unverified', command: setup.signIn, failed: !!failed }
}

/** `chatAgents` the host supports, restricted to the ones listed here. */
export function onboardingAgents(chatAgents) {
  const supported = Array.isArray(chatAgents) ? chatAgents : ['codex', 'claude']
  return Object.keys(AGENT_SETUP).filter(id => supported.includes(id))
}

export function agentSummary(steps) {
  const list = steps.filter(Boolean)
  return {
    anyReady: list.some(s => s.state === 'ready'),
    anyInstalled: list.some(s => s.state !== 'install'),
  }
}

// ---- rendering (popup and side panel) -------------------------------------

function el(document, tag, text, className) {
  const node = document.createElement(tag)
  if (text !== undefined) node.textContent = text
  if (className) node.className = className
  return node
}

function copyButton(document, t, text) {
  const copy = el(document, 'button', t('panel_copy_command'), 'onboard-copy')
  copy.type = 'button'
  copy.addEventListener('click', async () => {
    try { await globalThis.navigator.clipboard.writeText(text); copy.textContent = t('panel_copied') }
    catch { copy.textContent = t('panel_copy_failed') }
  })
  return copy
}

function commandBlock(document, t, command) {
  const row = el(document, 'div', undefined, 'onboard-command')
  row.append(el(document, 'code', command), copyButton(document, t, command))
  return row
}

function openUrl(chrome, url) {
  if (chrome?.tabs?.create) chrome.tabs.create({ url })
  else globalThis.open?.(url, '_blank')
}

/**
 * Fill `container` with the host setup guide for `plan`. Mac downloads go
 * through chrome.downloads; a failed one (the release has no disk image yet,
 * or the network) falls back to the terminal line.
 */
export function renderHostSetup(container, plan, { document, t, chrome }) {
  container.replaceChildren()
  container.append(el(document, 'p', t('panel_onboard_title'), 'onboard-title'), el(document, 'p', t('panel_onboard_intro'), 'onboard-intro'))
  const steps = el(document, 'ol', undefined, 'onboard-steps')
  for (const key of plan.steps) steps.append(el(document, 'li', t(key)))
  container.append(steps)
  const status = el(document, 'p', '', 'onboard-status')
  status.setAttribute('role', 'status')
  const fallback = el(document, 'details', undefined, 'onboard-fallback')
  fallback.append(el(document, 'summary', t(plan.os === 'mac' ? 'panel_onboard_terminal_instead' : 'panel_onboard_terminal_command')), commandBlock(document, t, plan.command),
    // The CLI is there but not registered for this Chrome (one started with
    // --user-data-dir, say): registering again is enough.
    el(document, 'p', t('panel_onboard_already_installed'), 'onboard-intro'), commandBlock(document, t, 'chrome-use extension install'))
  if (plan.os !== 'mac') fallback.open = true
  if (plan.downloads.length) {
    const actions = el(document, 'div', undefined, 'onboard-actions')
    for (const download of plan.downloads) {
      const button = el(document, 'button', t(download.labelKey), 'onboard-primary')
      button.type = 'button'
      button.addEventListener('click', () => startDownload(download.url, { chrome, t, status, fallback, button }))
      actions.append(button)
    }
    container.append(actions)
  }
  container.append(status, fallback)
  const waiting = el(document, 'p', t('panel_onboard_waiting'), 'onboard-waiting')
  const docs = el(document, 'a', t('panel_onboard_help'), 'onboard-link')
  docs.href = INSTALL_DOCS
  docs.addEventListener('click', event => { event.preventDefault(); openUrl(chrome, INSTALL_DOCS) })
  container.append(waiting, docs)
}

function startDownload(url, { chrome, t, status, fallback, button }) {
  if (!chrome?.downloads?.download) { openUrl(chrome, url); return }
  button.disabled = true
  status.textContent = t('panel_onboard_downloading')
  const failed = () => { button.disabled = false; status.textContent = t('panel_onboard_download_failed'); fallback.open = true }
  chrome.downloads.download({ url, saveAs: false }).then(id => {
    const listener = delta => {
      if (delta.id !== id || !delta.state) return
      if (delta.state.current === 'complete') {
        chrome.downloads.onChanged.removeListener(listener)
        button.disabled = false
        status.textContent = t('panel_onboard_downloaded')
        const show = el(globalThis.document, 'button', t('panel_onboard_show_download'), 'onboard-copy')
        show.type = 'button'
        show.addEventListener('click', () => { try { chrome.downloads.show(id) } catch {} })
        status.append(' ', show)
      } else if (delta.state.current === 'interrupted') {
        chrome.downloads.onChanged.removeListener(listener)
        failed()
      }
    }
    chrome.downloads.onChanged.addListener(listener)
  }, failed)
}

/** Read the CPU architecture for the Mac download ('' when unknown). */
export async function readArchitecture(navigator = globalThis.navigator) {
  try {
    const values = await navigator?.userAgentData?.getHighEntropyValues?.(['architecture'])
    return typeof values?.architecture === 'string' ? values.architecture : ''
  } catch { return '' }
}

/** The platform string the pages pass around. */
export function currentPlatform(navigator = globalThis.navigator) {
  return navigator?.userAgentData?.platform || navigator?.platform || ''
}

const AGENT_STATE_KEYS = { ready: 'panel_onboard_agent_ready', signIn: 'panel_onboard_agent_sign_in', unverified: 'panel_onboard_agent_unverified', install: 'panel_onboard_agent_install' }

/**
 * Fill `container` with the agent step. `steps` comes from agentNextStep;
 * `connectedNow` says the host was just set up in this page.
 */
export function renderAgentSetup(container, steps, { document, t, chrome, connectedNow = false, onRecheck, onHide }) {
  container.replaceChildren()
  const summary = agentSummary(steps)
  const title = connectedNow ? t('panel_onboard_connected') : t('panel_onboard_agents_title')
  container.append(el(document, 'p', title, 'onboard-title'))
  container.append(el(document, 'p', summary.anyReady ? t('panel_onboard_agents_ready') : summary.anyInstalled ? t('panel_onboard_agents_sign_in') : t('panel_onboard_agents_none'), 'onboard-intro'))
  const list = el(document, 'ul', undefined, 'onboard-agents')
  for (const step of steps) {
    const item = el(document, 'li', undefined, `onboard-agent ${step.state}`)
    const head = el(document, 'div', undefined, 'onboard-agent-head')
    head.append(el(document, 'strong', step.name), el(document, 'span', t(AGENT_STATE_KEYS[step.state]), 'onboard-agent-state'))
    item.append(head)
    if (step.state === 'signIn' || step.state === 'unverified') {
      item.append(el(document, 'p', t(step.state === 'signIn' ? 'panel_onboard_agent_sign_in_how' : 'panel_onboard_agent_unverified_how'), 'onboard-agent-how'), commandBlock(document, t, step.command))
    } else if (step.state === 'install') {
      const link = el(document, 'a', t('panel_onboard_agent_install_link', step.name), 'onboard-link')
      link.href = step.url
      link.addEventListener('click', event => { event.preventDefault(); openUrl(chrome, step.url) })
      item.append(link)
    }
    list.append(item)
  }
  container.append(list)
  const actions = el(document, 'div', undefined, 'onboard-actions')
  const recheck = el(document, 'button', t('panel_onboard_recheck'), 'onboard-copy')
  recheck.type = 'button'
  recheck.addEventListener('click', () => onRecheck?.())
  const hide = el(document, 'button', t('panel_onboard_hide'), 'onboard-copy')
  hide.type = 'button'
  hide.addEventListener('click', () => onHide?.())
  actions.append(recheck, hide)
  container.append(actions)
}

const AGENTS_OK_KEY = 'ab_onboarding_agents_ok'
const AGENTS_HIDDEN_KEY = 'ab_onboarding_agents_hidden'

/**
 * The side panel's onboarding area: the host setup guide in `hostBox`, the
 * agent step in `agentBox`. `api(operation, params)` is the panel's host
 * request; `schedule`/`cancel` are the panel's timers. Call
 * `update(state, panelSupport)` from the panel's connection render; it
 * returns whether the setup guide is showing. `close()` stops the polling.
 *
 * Opening the panel never starts an agent CLI the user has not picked (see
 * sidepanel.test.js), so the agent step reads only `discover` (which looks
 * for executables) by itself. Sign-in is read with `agentInspect`, which runs
 * each CLI, only when the user presses "Check sign-in".
 *
 * It shows once per connection when the host was set up in this panel, or
 * when no supported agent is installed. A panel that already works sets
 * `ab_onboarding_agents_ok` the first time it finds an agent installed and
 * never looks again; "Hide" sets `ab_onboarding_agents_hidden`.
 */
export function createPanelOnboarding({ document, t, chrome, storage, hostBox, agentBox, api, schedule = globalThis.setTimeout, cancel = globalThis.clearTimeout }) {
  const tracker = new HostSetupTracker()
  let rendered = false
  let poll = null
  let checkedThisConnection = false
  let agentEpoch = 0
  let support = null
  let inspected = false
  const read = async () => { try { return (await storage?.local?.get?.([AGENTS_OK_KEY, AGENTS_HIDDEN_KEY])) || {} } catch { return {} } }
  const write = values => { try { void Promise.resolve(storage?.local?.set?.(values)).catch(() => {}) } catch {} }

  async function checkAgents({ inspect = inspected, auto = false } = {}) {
    if (!agentBox || !support) return
    const epoch = ++agentEpoch
    const ids = onboardingAgents(support.chatAgents)
    const result = await api('discover')
    const found = new Map((result?.agents || []).map(agent => [agent.id, agent]))
    const steps = await Promise.all(ids.map(async id => {
      const discovered = found.get(id)
      if (!discovered?.executable || !inspect) return agentNextStep(id, discovered)
      try { return agentNextStep(id, discovered, (await api('agentInspect', { agentId: id }))?.inspection) }
      catch { return agentNextStep(id, discovered, null, true) }
    }))
    if (epoch !== agentEpoch) return
    if (inspect) inspected = true
    const summary = agentSummary(steps)
    if (summary.anyInstalled) write({ [AGENTS_OK_KEY]: true })
    if (auto && summary.anyInstalled && !tracker.completed) { agentBox.hidden = true; return }
    renderAgentSetup(agentBox, steps.filter(Boolean), {
      document, t, chrome, connectedNow: tracker.completed,
      onRecheck: () => void checkAgents({ inspect: true }).catch(() => {}),
      onHide: () => { agentBox.hidden = true; write({ [AGENTS_HIDDEN_KEY]: true }) },
    })
    agentBox.hidden = false
  }

  // Back from a terminal where the user installed or signed in: look again.
  document?.addEventListener?.('visibilitychange', () => {
    if (document.visibilityState === 'visible' && agentBox && !agentBox.hidden) void checkAgents().catch(() => {})
  })

  function retry() {
    poll = schedule(() => {
      if (!poll) return
      try { void Promise.resolve(chrome.runtime.sendMessage({ type: 'ab-host-retry' })).catch(() => {}) } catch {}
      retry()
    }, HOST_POLL_MS)
  }
  function stopPolling() { if (poll) { cancel(poll); poll = null } }

  return {
    update(state, panelSupport) {
      const active = tracker.update(state)
      if (hostBox) {
        hostBox.hidden = !active
        if (active && !rendered) {
          rendered = true
          void readArchitecture().then(architecture => renderHostSetup(hostBox, hostSetupPlan({ platform: currentPlatform(), architecture }), { document, t, chrome }))
        }
      }
      if (active && !poll && chrome?.runtime?.sendMessage) retry()
      else if (!active) stopPolling()
      const usable = state?.connected === true && panelSupport?.mode === 'full'
      if (!usable) {
        checkedThisConnection = false
        support = null
        if (agentBox) agentBox.hidden = true
        return active
      }
      support = panelSupport
      if (!checkedThisConnection && agentBox) {
        checkedThisConnection = true
        void read().then(flags => {
          if (flags[AGENTS_HIDDEN_KEY]) return
          if (flags[AGENTS_OK_KEY] && !tracker.completed) return
          return checkAgents({ auto: true })
        }).catch(() => { agentBox.hidden = true })
      }
      return active
    },
    close: stopPolling,
  }
}
