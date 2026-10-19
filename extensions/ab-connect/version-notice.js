// What the popup and side panel say about versions: an outdated or too-old
// CLI, a new CLI waiting to take over, a failed background upgrade, and this
// extension's own pending update. Kept free of `chrome.*` and the DOM so the
// rules are unit-testable; the pages turn each notice into a banner.

/**
 * The oldest CLI this extension works well with. Below it the banner says the
 * CLI must be updated rather than merely that a newer one exists. Raise it when
 * the extension starts depending on a newer host.
 */
export const MIN_CLI_VERSION = '1.5.235';

/** How long "updated to X" stays after the extension updated. */
export const UPDATED_NOTICE_MS = 24 * 60 * 60 * 1000;

/** Manual commands for when the CLI cannot replace itself. */
export const MANUAL_UPGRADE_COMMAND = 'chrome-use upgrade';
export const WINDOWS_UPGRADE_COMMAND = 'irm https://raw.githubusercontent.com/leeguooooo/chrome-use/main/install.ps1 | iex';
/** For a CLI too old to report its version: reinstalling from the release is
 *  the one path that works for every old build (early `upgrade` commands did
 *  not install this project's release). */
export const INSTALL_COMMAND = 'curl -fsSL https://raw.githubusercontent.com/leeguooooo/chrome-use/main/install.sh | sh';

/** The reinstall line for this machine, from navigator's platform string. */
export function installCommand(platform) {
  return /win/i.test(String(platform || '')) && !/darwin/i.test(String(platform || '')) ? WINDOWS_UPGRADE_COMMAND : INSTALL_COMMAND;
}

const SELF_UPGRADE = ['ready', 'unwritable', 'windows', 'disabled'];

function parse(version) {
  if (typeof version !== 'string') return null;
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/.exec(version.trim());
  return m ? { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4] || null } : null;
}

/** -1 / 0 / 1 like a comparator; null when either side is not a version.
 *  A release outranks its own pre-releases (semver), which compare as text. */
export function compareVersions(a, b) {
  const x = parse(a), y = parse(b);
  if (!x || !y) return null;
  for (let i = 0; i < 3; i++) if (x.core[i] !== y.core[i]) return x.core[i] < y.core[i] ? -1 : 1;
  if (x.pre === y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  return x.pre < y.pre ? -1 : 1;
}

const newer = (a, b) => compareVersions(a, b) === 1;

/** The `cli` record a host reports in its pong, validated; null when absent
 *  or malformed (an older host), so nothing CLI-related is shown. */
export function parseCliStatus(raw) {
  if (!raw || typeof raw !== 'object' || !parse(raw.version)) return null;
  const version = (v) => (parse(v) ? v : null);
  const last = raw.lastAutoUpgrade;
  return {
    version: raw.version,
    latest: version(raw.latest),
    onDisk: version(raw.onDisk),
    autoUpgrade: raw.autoUpgrade === true,
    selfUpgrade: SELF_UPGRADE.includes(raw.selfUpgrade) ? raw.selfUpgrade : 'disabled',
    lastAutoUpgrade:
      last && typeof last === 'object' && Number.isFinite(last.at) && typeof last.ok === 'boolean'
        ? { at: last.at, ok: last.ok, ...(typeof last.error === 'string' && last.error ? { error: last.error.slice(0, 300) } : {}) }
        : null,
  };
}

/** Whether the installed binary is newer than the host process running now. */
export function cliSwitchPending(cli) {
  return !!cli && newer(cli.onDisk, cli.version);
}

/** The command a user runs by hand when the CLI cannot replace itself. */
export function manualCommand(cli) {
  return cli?.selfUpgrade === 'windows' ? WINDOWS_UPGRADE_COMMAND : MANUAL_UPGRADE_COMMAND;
}

/** Whether the "Update CLI" button can do the work (otherwise show the command). */
export function canSelfUpgrade(cli) {
  return cli?.selfUpgrade === 'ready' || cli?.selfUpgrade === 'disabled';
}

/**
 * The notices to show, most important first. Each is
 * `{ id, level: 'warn'|'info', key, args, detail?, action?, command?, disabled?, reasonKey? }`:
 * `key`/`args` name the message, `detail` is raw text (an error) shown under
 * it, `action` the button ('upgradeCli' or 'applyUpdate'), `command` a line to
 * run by hand.
 *
 * - `cli`: parseCliStatus output, or null.
 * - `legacyCli`: the host answered without a `cli` record (a CLI from before
 *   it); `platform` picks the reinstall line shown for it.
 * - `cliUpgrade`: the worker's record of a button-started upgrade,
 *   `{ state: 'running'|'failed', error?, command? }`, or null.
 * - `update`: the worker's `updateStateSnapshot()` plus `canApplyNow`, or null.
 */
export function versionNotices({ cli = null, legacyCli = false, platform = '', cliUpgrade = null, update = null, extensionVersion = null, now = Date.now() } = {}) {
  const notices = [];
  if (!cli && legacyCli) {
    notices.push({ id: 'cli-legacy', level: 'warn', key: 'ext_version_cli_legacy', args: [], command: installCommand(platform) });
  } else if (cli) {
    const command = manualCommand(cli);
    const button = canSelfUpgrade(cli) ? { action: 'upgradeCli' } : { command };
    if (cliSwitchPending(cli)) {
      notices.push({ id: 'cli-switch', level: 'info', key: 'ext_version_cli_switch', args: [cli.onDisk] });
    } else if (cliUpgrade?.state === 'running') {
      notices.push({ id: 'cli-upgrading', level: 'info', key: 'ext_version_cli_upgrading', args: [] });
    } else if (cliUpgrade?.state === 'failed') {
      notices.push({
        id: 'cli-failed', level: 'warn', key: 'ext_version_cli_failed', args: [], ...(cliUpgrade.error ? { detail: cliUpgrade.error } : {}),
        command: cliUpgrade.command || command, ...(canSelfUpgrade(cli) ? { action: 'upgradeCli' } : {}),
      });
    } else if (compareVersions(cli.version, MIN_CLI_VERSION) === -1) {
      notices.push({ id: 'cli-too-old', level: 'warn', key: 'ext_version_cli_too_old', args: [cli.version, cli.latest || MIN_CLI_VERSION], ...button });
    } else if (newer(cli.latest, cli.version)) {
      const failed = cli.lastAutoUpgrade && !cli.lastAutoUpgrade.ok;
      notices.push(failed
        ? { id: 'cli-auto-failed', level: 'warn', key: 'ext_version_cli_auto_failed', args: [cli.latest], ...(cli.lastAutoUpgrade.error ? { detail: cli.lastAutoUpgrade.error } : {}), ...button, ...(button.action ? { command } : {}) }
        : { id: 'cli-outdated', level: cli.autoUpgrade && cli.selfUpgrade === 'ready' ? 'info' : 'warn', key: 'ext_version_cli_outdated', args: [cli.latest, cli.version], ...button });
    }
  }
  if (update?.pending) {
    const version = update.version || '';
    notices.push(update.canApplyNow
      ? { id: 'ext-pending', level: 'info', key: 'ext_version_ext_pending', args: [version], action: 'applyUpdate' }
      : { id: 'ext-pending', level: 'info', key: 'ext_version_ext_pending', args: [version], action: 'applyUpdate', disabled: true, reasonKey: holdReasonKey(update.reason) });
  } else if (update?.installed?.reason === 'update' && (!extensionVersion || update.installed.version === extensionVersion)
    && Number.isFinite(update.installed.at) && now - update.installed.at < UPDATED_NOTICE_MS) {
    notices.push({ id: 'ext-updated', level: 'info', key: 'ext_version_ext_updated', args: [update.installed.version || ''] });
  }
  return notices;
}

/** Why a downloaded extension update has to wait, as a message key. */
export function holdReasonKey(reason) {
  if (reason === 'command_in_flight') return 'ext_version_hold_busy';
  if (reason === 'applying') return 'ext_version_hold_applying';
  return 'ext_version_hold_recent';
}
