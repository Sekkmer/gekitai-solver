// Exact 6 x 6 rules. Board owners stay absolute: 1 = X, 2 = O.
export const LINES = [];
for (let r = 0; r < 6; r++)
  for (let c = 0; c < 6; c++)
    for (const [dr, dc] of [
      [0, 1],
      [1, 0],
      [1, 1],
      [1, -1],
    ]) {
      if (r + 2 * dr < 6 && c + 2 * dc >= 0 && c + 2 * dc < 6)
        LINES.push([
          r * 6 + c,
          (r + dr) * 6 + c + dc,
          (r + 2 * dr) * 6 + c + 2 * dc,
        ]);
    }
export const TRANSFORMS = Array.from({ length: 8 }, (_, t) =>
  Array.from({ length: 36 }, (_, i) => {
    const r = Math.floor(i / 6),
      c = i % 6;
    const [a, b] = [
      [r, c],
      [c, 5 - r],
      [5 - r, 5 - c],
      [5 - c, r],
      [r, 5 - c],
      [5 - r, c],
      [c, r],
      [5 - c, 5 - r],
    ][t];
    return a * 6 + b;
  }),
);
const weights = Array.from({ length: 36 }, (_, i) => 3n ** BigInt(i));
const low = TRANSFORMS.map((map) =>
  map.map((i) => Number(weights[i] & 0xffffffffn)),
);
const high = TRANSFORMS.map((map) => map.map((i) => Number(weights[i] >> 32n)));
export const ORDER = Array.from({ length: 36 }, (_, i) => i).sort(
  (a, b) =>
    ((a % 6) - 2.5) ** 2 +
    (Math.floor(a / 6) - 2.5) ** 2 -
    (((b % 6) - 2.5) ** 2 + (Math.floor(b / 6) - 2.5) ** 2),
);
export function canonical(board, turn) {
  let hi = Infinity,
    lo = Infinity,
    transform = 0;
  for (let t = 0; t < 8; t++) {
    let l = 0,
      h = 0;
    for (let i = 0; i < 36; i++)
      if (board[i]) {
        const v = board[i] === turn ? 1 : 2;
        l += v * low[t][i];
        h += v * high[t][i];
      }
    h += Math.floor(l / 4294967296);
    l = l >>> 0;
    if (h < hi || (h === hi && l < lo)) {
      hi = h;
      lo = l;
      transform = t;
    }
  }
  return { hi, lo, transform, key: hi + ':' + lo + ':' + turn };
}
export function legal(board) {
  return ORDER.filter((i) => board[i] === 0);
}
export function conditions(board, owner) {
  const lines = LINES.filter((line) => line.every((i) => board[i] === owner));
  const count = board.reduce((n, v) => n + (v === owner), 0);
  return { lines, count, won: lines.length > 0 || count === 8 };
}
export function terminal(board) {
  const x = conditions(board, 1),
    o = conditions(board, 2);
  if (x.won && o.won)
    return { winner: 0, reason: 'Both players completed a win condition' };
  const winner = x.won ? 1 : o.won ? 2 : null;
  if (winner === null) return null;
  const win = winner === 1 ? x : o;
  return {
    winner,
    reason: win.lines.length
      ? 'Three adjacent pieces in a row'
      : 'All eight pieces are on the board',
    squares: win.lines.flat(),
  };
}
export function apply(board, turn, square, trace) {
  if (
    !Number.isInteger(square) ||
    square < 0 ||
    square >= 36 ||
    board[square] !== 0
  )
    throw Error('Choose an empty square');
  if (board.filter((v) => v === turn).length >= 8)
    throw Error('No pieces remain in supply');
  const next = board.slice();
  next[square] = turn;
  const r = Math.floor(square / 6),
    c = square % 6;
  for (let dr = -1; dr <= 1; dr++)
    for (let dc = -1; dc <= 1; dc++) {
      if (!dr && !dc) continue;
      const nr = r + dr,
        nc = c + dc;
      if (nr < 0 || nr >= 6 || nc < 0 || nc >= 6) continue;
      const n = nr * 6 + nc;
      if (!next[n]) continue;
      const rr = r + 2 * dr,
        cc = c + 2 * dc;
      if (rr < 0 || rr >= 6 || cc < 0 || cc >= 6) {
        trace?.push({ from: n, to: null, owner: next[n], dr, dc });
        next[n] = 0;
      } else if (!next[rr * 6 + cc]) {
        trace?.push({ from: n, to: rr * 6 + cc, owner: next[n], dr, dc });
        next[rr * 6 + cc] = next[n];
        next[n] = 0;
      }
    }
  return { board: next, result: terminal(next) };
}
export function resultText(result) {
  return result.winner === 0
    ? 'Draw'
    : (result.winner === 1 ? 'First player (X)' : 'Second player (O)') +
        ' wins';
}
export const DEMO = [
  14, 15, 21, 14, 15, 20, 15, 27, 21, 30, 13, 14, 21, 34, 28, 21, 26, 29, 15,
  14, 25, 19, 14, 15, 14,
];
