// The extension's optional http(s) host access. With it the extension takes
// focus out of a field on an agent tab when a password manager's inline menu
// blocks the agent (menu-blur.js), and picks elements without one Chrome
// prompt per site. Chrome grants it only from a click in the extension's own
// pages: the popup's button or the options page's switch. Few users found the
// switch, so a menu the extension could not close for want of it is reported
// once a day with a notification that opens the switch.

export const SITE_ACCESS = { origins: ['http://*/*', 'https://*/*'] }
export const SITE_ACCESS_NOTIFICATION = 'site-access-needed'
export const SITE_ACCESS_OPTIONS_PAGE = 'options.html#site-access'
const NOTIFIED_AT = 'ab_site_access_notified_at'
const NOTIFY_EVERY_MS = 24 * 60 * 60 * 1000

/** Whether the access is granted; null when Chrome cannot tell. */
export async function hasSiteAccess(permissions) {
  try {
    return (await permissions.contains(SITE_ACCESS)) === true
  } catch {
    return null
  }
}

/** Whether an error from blurFocusedField means the access is missing. */
export function isSiteAccessError(error) {
  return /no site access/.test(String(error?.message || error))
}

/**
 * Tell the user, at most once a day, that a menu stayed open for want of the
 * access. deps: { storage (chrome.storage.local), notifications, t, now }.
 * Returns whether a notification was shown.
 */
export async function notifySiteAccessNeeded({ storage, notifications, t, now = Date.now() }) {
  const { [NOTIFIED_AT]: last = -Infinity } = await storage.get(NOTIFIED_AT)
  if (now - last < NOTIFY_EVERY_MS) return false
  await storage.set({ [NOTIFIED_AT]: now })
  await notifications.create(SITE_ACCESS_NOTIFICATION, {
    type: 'basic',
    iconUrl: 'icons/icon128.png',
    title: t('bg_site_access_title'),
    message: t('bg_site_access_message'),
    priority: 1,
  })
  return true
}
