// Render version notices (version-notice.js) as compact banners. Shared by the
// popup and the side panel; each page styles `.version-notice` itself.

const ACTION_LABELS = { upgradeCli: 'ext_version_action_upgrade_cli', applyUpdate: 'ext_version_action_apply_update' };

/**
 * Replace `container`'s children with one banner per notice. `onAction(action)`
 * runs when a banner's button is clicked; it may return a promise, during
 * which the button stays disabled.
 */
export function renderVersionNotices(container, notices, { document, t, onAction }) {
  if (!container) return;
  const banners = (notices || []).map(notice => {
    const banner = document.createElement('div');
    banner.className = `version-notice ${notice.level}`;
    banner.dataset.notice = notice.id;
    const text = document.createElement('span');
    text.className = 'version-text';
    text.textContent = t(notice.key, ...(notice.args || []));
    banner.append(text);
    // The command right under "run this in a terminal", then the why.
    if (notice.command) {
      const code = document.createElement('code');
      code.textContent = notice.command;
      banner.append(code);
    }
    if (notice.detail) {
      const detail = document.createElement('span');
      detail.className = 'version-reason';
      detail.textContent = notice.detail;
      banner.append(detail);
    }
    if (notice.reasonKey) {
      const reason = document.createElement('span');
      reason.className = 'version-reason';
      reason.textContent = t(notice.reasonKey);
      banner.append(reason);
    }
    if (notice.action) {
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = t(ACTION_LABELS[notice.action]);
      button.disabled = !!notice.disabled;
      button.addEventListener('click', () => {
        button.disabled = true;
        Promise.resolve(onAction?.(notice.action)).catch(() => {}).finally(() => { button.disabled = !!notice.disabled; });
      });
      banner.append(button);
    }
    return banner;
  });
  container.replaceChildren(...banners);
  container.hidden = banners.length === 0;
}
