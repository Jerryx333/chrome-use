// Normalize user-selected bitmaps before they cross native-message limits.
// Image bytes live only in the draft until saved by the conversation service.
const BITMAP_TYPES = ['image/png','image/jpeg','image/webp'];
const HEIF_TYPES = ['image/heic','image/heif','image/heic-sequence','image/heif-sequence'];

// Windows and some pickers report an empty type for HEIC, so the extension counts too.
export function isHeif(file) {
  return HEIF_TYPES.includes(file.type) || /\.(heic|heif)$/i.test(file.name || '');
}

export function isSupportedImage(file) {
  return BITMAP_TYPES.includes(file.type) || isHeif(file);
}

function toBase64(bytes) {
  let binary = '';
  for (let index = 0; index < bytes.length; index += 0x8000) binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  return btoa(binary);
}

// Chrome has no HEIC decoder; the local CLI converts with the system codec
// and answers with a JPEG small enough for the host-to-extension message limit.
export function heifConverter(api) {
  return async file => {
    const result = await api('imageConvert', { data: toBase64(new Uint8Array(await file.arrayBuffer())) });
    return new Blob([Uint8Array.from(atob(result.data), character => character.charCodeAt(0))], { type: 'image/jpeg' });
  };
}

export async function prepareImage(file, { convertHeif = null } = {}) {
  if (!isSupportedImage(file)) throw new Error('Use PNG, JPEG, WebP or HEIC images.');
  if (file.size > 20 * 1024 * 1024) throw new Error('The source image exceeds 20 MiB.');
  let bitmap;
  try { bitmap = await createImageBitmap(file); }
  catch (error) {
    if (!isHeif(file)) throw error;
    if (!convertHeif) throw new Error('Update the chrome-use CLI to attach HEIC photos, or export them as JPEG first.');
    bitmap = await createImageBitmap(await convertHeif(file));
  }
  try {
    const scale = Math.min(1, 2000 / Math.max(bitmap.width, bitmap.height));
    const canvas = new OffscreenCanvas(Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale)));
    const context = canvas.getContext('2d');
    if (!context) throw new Error('Image conversion is unavailable.');
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    let blob = await canvas.convertToBlob({ type: 'image/png' });
    if (blob.size > 512 * 1024) {
      // White background preserves transparency legibility in JPEG.
      context.globalCompositeOperation = 'destination-over'; context.fillStyle = '#ffffff'; context.fillRect(0, 0, canvas.width, canvas.height);
      for (const quality of [0.85, 0.7, 0.5, 0.3]) {
        blob = await canvas.convertToBlob({ type: 'image/jpeg', quality });
        if (blob.size <= 512 * 1024) break;
      }
    }
    if (blob.size > 512 * 1024) throw new Error('This image is still too large; crop or resize it before attaching.');
    return { data: toBase64(new Uint8Array(await blob.arrayBuffer())), mediaType: blob.type, name: file.name || 'Image' };
  } finally { bitmap.close(); }
}
