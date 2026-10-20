import assert from 'node:assert/strict';
import test from 'node:test';
import { missingHostHint } from './native-host-hint.js';

test('missing native host provides an explanation and install command', () => {
  for (const error of ['Specified native messaging host not found.', 'HOST NOT FOUND']) {
    assert.deepEqual(missingHostHint(error), {
      messageKey: 'popup_hint_install_host', command: 'chrome-use extension install',
    });
  }
  for (const error of [undefined, '', 'Native host exited.', 'Access denied']) {
    assert.equal(missingHostHint(error), null);
  }
});
