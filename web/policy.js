import { canonical, TRANSFORMS, legal, apply } from './engine.js';
export function decodePolicy(raw, count) {
  const hi = new Uint32Array(count),
    lo = new Uint32Array(count),
    moves = new Uint8Array(count);
  let i = 0,
    h = 0,
    l = 0;
  for (let n = 0; n < count; n++) {
    let dl = 0,
      dh = 0,
      shift = 0,
      v;
    do {
      if (i >= raw.length || shift > 56)
        throw Error('Invalid strategy encoding');
      v = raw[i++];
      const digit = v & 127;
      // Split every delta into exact 32-bit words, including the initial 59-bit key.
      if (shift < 32) {
        const value = digit * 2 ** shift;
        dl += value % 4294967296;
        dh += Math.floor(value / 4294967296);
      } else dh += digit * 2 ** (shift - 32);
      shift += 7;
    } while (v & 128);
    l += dl;
    h += dh + Math.floor(l / 4294967296);
    l = l >>> 0;
    const move = raw[i++];
    if (
      move >= 36 ||
      move === undefined ||
      h > 0x7ffffff ||
      (n && (h < hi[n - 1] || (h === hi[n - 1] && l <= lo[n - 1])))
    )
      throw Error('Invalid strategy record');
    hi[n] = h;
    lo[n] = l;
    moves[n] = move;
  }
  if (i !== raw.length) throw Error('Unexpected strategy data');
  return { hi, lo, moves };
}
export function policyMove(policy, board) {
  const key = canonical(board, 1);
  // Certificate code = canonical ternary key + goal-turn bit + 1.
  const lo = (key.lo + 1) >>> 0,
    hi = key.hi + 0x4000000 + (lo === 0 ? 1 : 0);
  let a = 0,
    b = policy.moves.length;
  while (a < b) {
    const m = Math.floor((a + b) / 2);
    if (policy.hi[m] < hi || (policy.hi[m] === hi && policy.lo[m] < lo))
      a = m + 1;
    else b = m;
  }
  if (a < policy.moves.length && policy.hi[a] === hi && policy.lo[a] === lo) {
    const square = TRANSFORMS[key.transform].indexOf(policy.moves[a]);
    if (board[square] !== 0)
      throw Error('Strategy selected an occupied square');
    return square;
  }
  // The original certificate regenerates immediate winning leaves.
  for (const square of legal(board))
    if (apply(board, 1, square).result?.winner === 1) return square;
  throw Error('This position is outside the verified strategy');
}
