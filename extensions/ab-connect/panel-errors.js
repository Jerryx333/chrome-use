// The host refuses to replace an older build's panel service while one of its
// conversations is working or waiting for the user. Its error ends with
// `[blocking:<ids>]`; return those conversation ids, or null.
export function oldBuildBlocking(text) {
  const match = /\[blocking:([0-9a-f,-]+)\]\s*$/.exec(String(text || ''));
  if (!match) return null;
  const ids = match[1].split(',').filter(id => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id));
  return ids.length ? ids : null;
}
