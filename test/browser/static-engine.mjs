import assert from 'node:assert/strict';
import fs from 'node:fs';
import zlib from 'node:zlib';
import {
  apply,
  canonical,
  conditions,
  terminal,
  LINES,
  TRANSFORMS,
  DEMO,
  legal,
} from '../../web/engine.js';
import { decodePolicy, policyMove } from '../../web/policy.js';
import { analyze, verifyTree, bestEffort } from '../../web/practice.js';
assert.equal(LINES.length, 80);
let triples = 0;
for (let a = 0; a < 36; a++)
  for (let b = a + 1; b < 36; b++)
    for (let c = b + 1; c < 36; c++) {
      const board = Array(36).fill(0);
      for (const i of [a, b, c]) board[i] = 1;
      const coords = [a, b, c].map((i) => [Math.floor(i / 6), i % 6]);
      const [r, c0] = coords[0],
        [r1, c1] = coords[1],
        [r2, c2] = coords[2];
      const adjacent =
        r1 - r === r2 - r1 &&
        c1 - c0 === c2 - c1 &&
        Math.max(Math.abs(r1 - r), Math.abs(c1 - c0)) === 1;
      assert.equal(conditions(board, 1).won, adjacent);
      triples++;
    }
const fixtures = JSON.parse(
  fs.readFileSync(new URL('../evidence/static-rules.json', import.meta.url)),
);
for (const f of fixtures) {
  const trace = [],
    a = apply(f.board, f.turn, f.square, trace);
  assert.deepEqual(a.board, f.next);
  const replay = f.board.slice();
  replay[f.square] = f.turn;
  for (const move of trace) {
    assert.equal(replay[move.from], move.owner);
    replay[move.from] = 0;
    if (move.to !== null) {
      assert.equal(replay[move.to], 0);
      replay[move.to] = move.owner;
    }
  }
  assert.deepEqual(replay, f.next);
  assert.equal(a.result?.winner ?? null, f.winner);
  const key = canonical(f.board, f.turn);
  assert.equal(((BigInt(key.hi) << 32n) | BigInt(key.lo)).toString(), f.key);
}
const meta = JSON.parse(
  fs.readFileSync(new URL('../../web/assets/strategy.json', import.meta.url)),
);
const raw = zlib.gunzipSync(
  fs.readFileSync(new URL('../../web/assets/' + meta.file, import.meta.url)),
);
const policy = decodePolicy(raw, meta.entries);
// A separate BigInt decoder verifies every split-word decoded key and move.
let offset = 0,
  code = 0n;
for (let n = 0; n < meta.entries; n++) {
  let d = 0n,
    shift = 0n,
    v;
  do {
    v = raw[offset++];
    d |= BigInt(v & 127) << shift;
    shift += 7n;
  } while (v & 128);
  code += d;
  assert.equal((BigInt(policy.hi[n]) << 32n) | BigInt(policy.lo[n]), code);
  assert.equal(policy.moves[n], raw[offset++]);
}
assert.equal(offset, raw.length);
let board = Array(36).fill(0),
  result;
for (let i = 0; i < DEMO.length; i++) {
  if (i % 2 === 0) assert.equal(policyMove(policy, board), DEMO[i]);
  ({ board, result } = apply(board, (i % 2) + 1, DEMO[i]));
  assert.equal(!!result, i === 24);
}
assert.equal(result.winner, 1);
assert.equal(result.reason, 'All eight pieces are on the board');
// Many complete adversarial paths, with rotations/reflections exercised naturally.
let seed = 87531;
const random = () => {
  seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
  return seed;
};
for (let game = 0; game < 500; game++) {
  let b = Array(36).fill(0),
    end = null;
  const path = new Set();
  for (let ply = 0; !end && ply < 25; ply++) {
    const turn = (ply % 2) + 1,
      key = canonical(b, turn).key;
    assert(!path.has(key));
    path.add(key);
    let square =
      turn === 1 ? policyMove(policy, b) : legal(b)[random() % legal(b).length];
    ({ board: b, result: end } = apply(b, turn, square));
  }
  assert.equal(end?.winner, 1);
}
// Immediate win and a retained two-ply proof checked against every human reply.
board = Array(36).fill(0);
board[0] = board[6] = 2;
board[22] = 1;
const answer = analyze(board, [canonical(board, 2).key], 300);
assert(answer.proof);
verifyTree(board, 2, answer.proof, []);
assert.equal(apply(board, 2, answer.square).result.winner, 2);
const tampered = structuredClone(answer.proof);
tampered.move = 0;
assert.throws(() => verifyTree(board, 2, tampered, []));
board = Array(36).fill(0);
for (const i of [0, 1, 28, 29]) board[i] = 2;
const tree = { rank: 2, replies: {} };
for (const sq of legal(board)) {
  const a = apply(board, 1, sq);
  if (a.result) {
    assert.equal(a.result.winner, 2);
    tree.replies[sq] = null;
    continue;
  }
  const reply = legal(a.board).find(
    (s) => apply(a.board, 2, s).result?.winner === 2,
  );
  assert.notEqual(reply, undefined);
  tree.replies[sq] = { rank: 1, move: reply, child: null };
}
verifyTree(board, 1, tree, []);
const missing = structuredClone(tree);
delete missing.replies[legal(board)[0]];
assert.throws(() => verifyTree(board, 1, missing, []));
for (const sq of legal(board)) {
  const a = apply(board, 1, sq);
  if (a.result) continue;
  const retained = analyze(
    a.board,
    [canonical(board, 1).key, canonical(a.board, 2).key],
    300,
    tree.replies[sq],
  );
  assert.equal(apply(a.board, 2, retained.square).result.winner, 2);
}
console.log(
  `Static engine passed: ${triples} triples, ${fixtures.length} Rust transitions/keys, all ${meta.entries} decoded records, demo policy replay, 500 complete strategy games, checked tactical proof and all retained replies.`,
);
