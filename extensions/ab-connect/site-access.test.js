import { test } from 'node:test'
import assert from 'node:assert/strict'
import { SITE_ACCESS, SITE_ACCESS_NOTIFICATION, hasSiteAccess, isSiteAccessError, notifySiteAccessNeeded } from './site-access.js'

function fakeStorage(initial = {}) {
  const data = { ...initial }
  return { data, get: async (key) => (key in data ? { [key]: data[key] } : {}), set: async (o) => { Object.assign(data, o) } }
}

test('hasSiteAccess asks Chrome for the http(s) origins', async () => {
  let asked = null
  assert.equal(await hasSiteAccess({ contains: async (p) => { asked = p; return true } }), true)
  assert.deepEqual(asked, SITE_ACCESS)
  assert.equal(await hasSiteAccess({ contains: async () => false }), false)
  assert.equal(await hasSiteAccess({ contains: async () => { throw new Error('no') } }), null)
})

test('isSiteAccessError recognises the blur refusal', () => {
  assert.equal(isSiteAccessError(new Error('blurFocusedField: no site access (Cannot access contents of the page)')), true)
  assert.equal(isSiteAccessError(new Error('blurFocusedField: the tab has no web frame to script')), false)
})

test('the notification shows at most once a day and names its id', async () => {
  const storage = fakeStorage()
  const shown = []
  const deps = { storage, notifications: { create: async (id, o) => { shown.push([id, o]) } }, t: (key) => key }
  assert.equal(await notifySiteAccessNeeded({ ...deps, now: 1000 }), true)
  assert.equal(await notifySiteAccessNeeded({ ...deps, now: 1000 + 60 * 60 * 1000 }), false)
  assert.equal(await notifySiteAccessNeeded({ ...deps, now: 1000 + 25 * 60 * 60 * 1000 }), true)
  assert.equal(shown.length, 2)
  assert.equal(shown[0][0], SITE_ACCESS_NOTIFICATION)
  assert.equal(shown[0][1].title, 'bg_site_access_title')
})
