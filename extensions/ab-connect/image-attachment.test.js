import assert from 'node:assert/strict';
import test from 'node:test';
import { heifConverter, isHeif, isSupportedImage, prepareImage } from './image-attachment.js';

// Node has no image decoder; these stubs stand in for Chrome's, which cannot
// decode HEIC and so throws for it.
const decoded = [];
globalThis.createImageBitmap = async source => {
  if (source.heic) throw new Error('The source image could not be decoded.');
  decoded.push(source);
  return { width: 4000, height: 3000, close() {} };
};
globalThis.OffscreenCanvas = class {
  constructor(width, height) { this.width = width; this.height = height; }
  getContext() { return { drawImage() {}, fillRect() {} }; }
  async convertToBlob({ type }) { return new Blob([new Uint8Array([1, 2, 3])], { type }); }
};
const heic = (name, type = '') => ({ name, type, size: 12, heic: true, async arrayBuffer() { return new Uint8Array([0, 0, 0, 24, 102, 116, 121, 112]).buffer; } });

test('HEIC is recognized by MIME type or, when the type is empty, by extension', () => {
  for (const type of ['image/heic', 'image/heif', 'image/heic-sequence', 'image/heif-sequence']) assert.ok(isHeif({ name: 'photo', type }));
  assert.ok(isHeif({ name: 'IMG_0001.HEIC', type: '' }));
  assert.ok(isHeif({ name: 'scan.heif', type: 'application/octet-stream' }));
  assert.ok(!isHeif({ name: 'photo.jpg', type: 'image/jpeg' }));
  assert.ok(isSupportedImage({ name: 'a.webp', type: 'image/webp' }));
  assert.ok(!isSupportedImage({ name: 'a.gif', type: 'image/gif' }));
});

test('unsupported types are rejected with the accepted list', async () => {
  await assert.rejects(prepareImage({ name: 'a.gif', type: 'image/gif', size: 1 }), /PNG, JPEG, WebP or HEIC/);
});

test('HEIC Chrome cannot decode goes through the host converter and is normalized', async () => {
  const calls = [];
  const convert = heifConverter(async (operation, params) => { calls.push({ operation, params }); return { mediaType: 'image/jpeg', data: btoa('JPEGDATA') }; });
  const image = await prepareImage(heic('IMG_0001.HEIC'), { convertHeif: convert });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].operation, 'imageConvert');
  assert.deepEqual(Object.keys(calls[0].params), ['data']);
  assert.equal(atob(calls[0].params.data).slice(4, 8), 'ftyp');
  const converted = decoded.at(-1);
  assert.equal(converted.type, 'image/jpeg');
  assert.equal(await converted.text(), 'JPEGDATA');
  assert.deepEqual(image, { data: btoa('\x01\x02\x03'), mediaType: 'image/png', name: 'IMG_0001.HEIC' });
});

test('a host without HEIC conversion yields an actionable message', async () => {
  await assert.rejects(prepareImage(heic('IMG_0001.HEIC', 'image/heic')), /Update the chrome-use CLI to attach HEIC photos/);
});

test('HEIC Chrome can decode itself never reaches the host', async () => {
  const image = await prepareImage({ name: 'photo.heic', type: 'image/heic', size: 12 }, { convertHeif: () => assert.fail('converter called') });
  assert.equal(image.mediaType, 'image/png');
});
