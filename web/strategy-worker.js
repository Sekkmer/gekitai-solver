import { decodePolicy, policyMove } from './policy.js';
let policy, loading;
const tell = (id, data) => postMessage({ id, ...data });
async function load(id) {
  const response = await fetch('./assets/strategy.json');
  if (!response.ok) throw Error('Could not load the strategy description');
  const meta = await response.json();
  if (
    meta.format !== 1 ||
    meta.entries !== 5312888 ||
    meta.decodedBytes !== 15075697 ||
    !/^strategy-[a-f0-9]{12}\.bin\.gz$/.test(meta.file)
  )
    throw Error('Unsupported strategy');
  const url = new URL('./assets/' + meta.file, import.meta.url).href;
  let cache;
  try {
    cache = await caches.open('gekitai-strategy-v1');
  } catch {
    /* Ordinary fetching still works without cache storage. */
  }
  let responseData = await cache?.match(url),
    cached = !!responseData;
  if (!responseData) responseData = await fetch(url);
  if (!responseData.ok)
    throw Error('Could not download the strategy. Try again when connected.');
  tell(id, {
    progress: cached
      ? 'Opening the saved strategy…'
      : 'Downloading the strategy · 12 MB…',
  });
  const compressed = await responseData.arrayBuffer();
  const digest = Array.from(
    new Uint8Array(await crypto.subtle.digest('SHA-256', compressed)),
    (v) => v.toString(16).padStart(2, '0'),
  ).join('');
  if (compressed.byteLength !== meta.bytes || digest !== meta.sha256) {
    await cache?.delete(url);
    throw Error('Strategy download failed its integrity check. Please retry.');
  }
  tell(id, { progress: 'Preparing the strategy…' });
  const bytes = new Uint8Array(
    await new Response(
      new Blob([compressed])
        .stream()
        .pipeThrough(new DecompressionStream('gzip')),
    ).arrayBuffer(),
  );
  if (bytes.length !== meta.decodedBytes)
    throw Error('Strategy size does not match');
  const decoded = decodePolicy(bytes, meta.entries);
  if (!cached)
    try {
      await cache?.put(
        url,
        new Response(compressed, {
          headers: { 'Content-Type': 'application/gzip' },
        }),
      );
    } catch {
      /* Storage limits do not prevent play. */
    }
  return decoded;
}
onmessage = async ({ data: { id, type, board } }) => {
  try {
    if (!policy) {
      loading ??= load(id).catch((error) => {
        loading = null;
        throw error;
      });
      policy = await loading;
    }
    tell(
      id,
      type === 'load' ? { ready: true } : { square: policyMove(policy, board) },
    );
  } catch (error) {
    tell(id, { error: error.message });
  }
};
