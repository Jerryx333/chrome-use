import assert from 'node:assert/strict'
import test from 'node:test'
import { detectOs, hostSetupPlan, needsHostSetup, HostSetupTracker, agentNextStep, onboardingAgents, agentSummary, macDownloadUrl, readArchitecture, RELEASE_DOWNLOAD } from './onboarding.js'
import { INSTALL_COMMAND, WINDOWS_UPGRADE_COMMAND } from './version-notice.js'

const missing = { connected: false, connectionState: 'disconnected', connectionError: 'Specified native messaging host not found.' }

test('platform strings map to the three setup paths', () => {
  assert.equal(detectOs('macOS'), 'mac')
  assert.equal(detectOs('MacIntel'), 'mac')
  assert.equal(detectOs('Windows'), 'windows')
  assert.equal(detectOs('Win32'), 'windows')
  assert.equal(detectOs('Linux x86_64'), 'linux')
  assert.equal(detectOs(''), 'linux')
})

test('a Mac gets the disk image for its CPU, both when the CPU is unknown', () => {
  const arm = hostSetupPlan({ platform: 'macOS', architecture: 'arm' })
  assert.deepEqual(arm.downloads.map(d => d.url), [`${RELEASE_DOWNLOAD}/chrome-use-darwin-arm64.dmg`])
  assert.equal(arm.downloads[0].labelKey, 'panel_onboard_download_mac')
  assert.equal(arm.command, INSTALL_COMMAND)
  const intel = hostSetupPlan({ platform: 'macOS', architecture: 'x86' })
  assert.deepEqual(intel.downloads.map(d => d.arch), ['x64'])
  const unknown = hostSetupPlan({ platform: 'MacIntel' })
  assert.deepEqual(unknown.downloads.map(d => d.labelKey), ['panel_onboard_download_mac_arm', 'panel_onboard_download_mac_intel'])
  assert.equal(macDownloadUrl('x64'), `${RELEASE_DOWNLOAD}/chrome-use-darwin-x64.dmg`)
  // The release asset names release-binaries.yml attaches.
  assert.match(macDownloadUrl('arm64'), /\/releases\/latest\/download\/chrome-use-darwin-arm64\.dmg$/)
})

test('Windows and Linux get a command to paste, no download', () => {
  const win = hostSetupPlan({ platform: 'Windows' })
  assert.deepEqual(win.downloads, [])
  assert.equal(win.command, WINDOWS_UPGRADE_COMMAND)
  const linux = hostSetupPlan({ platform: 'Linux' })
  assert.equal(linux.command, INSTALL_COMMAND)
  assert.equal(linux.steps.length, 3)
})

test('only a missing native host starts the guide', () => {
  assert.equal(needsHostSetup(missing), true)
  assert.equal(needsHostSetup({ ...missing, connectionError: 'Native host has exited.' }), false)
  assert.equal(needsHostSetup({ ...missing, connectionState: 'connecting' }), false)
  assert.equal(needsHostSetup({ connected: true }), false)
  assert.equal(needsHostSetup({ ...missing, standalone: true }), false)
  assert.equal(needsHostSetup(null), false)
})

test('the guide stays up through retries and reports completion once', () => {
  const tracker = new HostSetupTracker()
  assert.equal(tracker.update({ connected: false, connectionState: 'disconnected', connectionError: 'other' }), false)
  assert.equal(tracker.update(missing), true)
  assert.equal(tracker.update({ connected: false, connectionState: 'connecting' }), true)
  assert.equal(tracker.completed, false)
  assert.equal(tracker.update({ connected: false, connectionState: 'disconnected', connectionError: 'Access denied' }), false)
  assert.equal(tracker.update(missing), true)
  assert.equal(tracker.update({ connected: true, connectionState: 'connected' }), false)
  assert.equal(tracker.completed, true)
  // A user who was never shown the guide never "completes" it.
  const quiet = new HostSetupTracker()
  quiet.update({ connected: true })
  assert.equal(quiet.completed, false)
})

test('each agent gets its next step from discover and agentInspect', () => {
  assert.deepEqual(agentNextStep('codex', { executable: null }), { id: 'codex', name: 'Codex', state: 'install', url: 'https://developers.openai.com/codex/cli' })
  assert.equal(agentNextStep('codex', undefined).state, 'install')
  const out = agentNextStep('codex', { executable: '/x/codex' }, { account: { status: 'reportedSignedOut' } })
  assert.deepEqual(out, { id: 'codex', name: 'Codex', state: 'signIn', command: 'codex login' })
  assert.deepEqual(agentNextStep('claude', { executable: '/x/claude' }, { account: { status: 'reportedSignedIn', kind: 'claude.ai' } }), { id: 'claude', name: 'Claude Code', state: 'ready', kind: 'claude.ai' })
  const unknown = agentNextStep('opencode', { executable: '/x/opencode' }, null, true)
  assert.equal(unknown.state, 'unverified')
  assert.equal(unknown.command, 'opencode auth login')
  assert.equal(unknown.failed, true)
  assert.equal(agentNextStep('gemini', { executable: '/x' }), null)
})

test('the list follows what the host can chat with', () => {
  assert.deepEqual(onboardingAgents(['codex', 'claude', 'opencode', 'pi', 'gemini']), ['codex', 'claude', 'opencode', 'pi'])
  assert.deepEqual(onboardingAgents(undefined), ['codex', 'claude'])
  assert.deepEqual(agentSummary([{ state: 'install' }, { state: 'signIn' }, null]), { anyReady: false, anyInstalled: true })
  assert.deepEqual(agentSummary([{ state: 'install' }]), { anyReady: false, anyInstalled: false })
  assert.equal(agentSummary([{ state: 'ready' }]).anyReady, true)
})

test('the CPU architecture comes from userAgentData, empty when hidden', async () => {
  assert.equal(await readArchitecture({ userAgentData: { getHighEntropyValues: async () => ({ architecture: 'arm' }) } }), 'arm')
  assert.equal(await readArchitecture({ userAgentData: { getHighEntropyValues: async () => { throw new Error('no') } } }), '')
  assert.equal(await readArchitecture({}), '')
})
