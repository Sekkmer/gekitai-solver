import { LINES, legal, apply, canonical } from './engine.js';
const WIN = 30000,
  STOP = Symbol('deadline');
function evaluate(board, turn) {
  const other = 3 - turn;
  let score = 0;
  for (const v of board) score += v === turn ? 12 : v === other ? -12 : 0;
  for (const line of LINES) {
    let us = 0,
      them = 0;
    for (const i of line) {
      us += board[i] === turn;
      them += board[i] === other;
    }
    if (!them) score += [0, 1, 9, 50][us];
    if (!us) score -= [0, 1, 9, 50][them];
  }
  return score;
}
function terminalScore(result, turn, ply) {
  return result.winner === 0
    ? 0
    : result.winner === turn
      ? WIN - ply
      : -WIN + ply;
}
function ordered(board, turn) {
  return legal(board)
    .map((square) => {
      const a = apply(board, turn, square);
      return {
        square,
        ...a,
        score: a.result
          ? terminalScore(a.result, turn, 0)
          : evaluate(a.board, turn),
      };
    })
    .sort((a, b) => b.score - a.score);
}
export function bestEffort(board, history, deadline) {
  const moves = ordered(board, 2);
  let best = moves[0].square,
    completed = 0;
  function search(board, turn, depth, ply, alpha, beta, path) {
    if (performance.now() >= deadline) throw STOP;
    const key = canonical(board, turn).key;
    if (path.has(key)) return 0;
    if (!depth) return evaluate(board, turn);
    path.add(key);
    let best = -WIN;
    try {
      for (const a of ordered(board, turn)) {
        const score = a.result
          ? terminalScore(a.result, turn, ply)
          : -search(a.board, 3 - turn, depth - 1, ply + 1, -beta, -alpha, path);
        best = Math.max(best, score);
        alpha = Math.max(alpha, score);
        if (alpha >= beta) break;
      }
    } finally {
      path.delete(key);
    }
    return best;
  }
  for (let depth = 1; depth <= 16; depth++) {
    let alpha = -WIN,
      candidate = best;
    try {
      for (const a of moves) {
        if (performance.now() >= deadline) throw STOP;
        const score = a.result
          ? terminalScore(a.result, 2, 0)
          : -search(a.board, 1, depth - 1, 1, -WIN, -alpha, new Set(history));
        if (score > alpha) {
          alpha = score;
          candidate = a.square;
        }
      }
    } catch (e) {
      if (e !== STOP) throw e;
      break;
    }
    best = candidate;
    completed = depth;
    if (Math.abs(alpha) > WIN - 100) break;
    const index = moves.findIndex((a) => a.square === best);
    [moves[0], moves[index]] = [moves[index], moves[0]];
  }
  return { square: best, depth: completed };
}
// Small explicit AND/OR tree. No heuristic value or transposition cache can be a proof.
export function forcingTree(board, history, deadline) {
  const rootKey = canonical(board, 2).key,
    prior = new Set(history);
  prior.delete(rootKey);
  let visits = 0;
  function solve(board, turn, depth, path) {
    if (++visits > 200000 || performance.now() >= deadline) throw STOP;
    const key = canonical(board, turn).key;
    if (!depth || path.has(key)) return null;
    path.add(key);
    try {
      if (turn === 2) {
        for (const a of ordered(board, turn)) {
          if (a.result) {
            if (a.result.winner === 2)
              return { move: a.square, child: null, rank: 1 };
            continue;
          }
          const child = solve(a.board, 1, depth - 1, path);
          if (child) return { move: a.square, child, rank: child.rank + 1 };
        }
        return null;
      }
      const replies = {},
        moves = ordered(board, turn);
      let rank = 1;
      for (const a of moves) {
        if (a.result) {
          if (a.result.winner !== 2) return null;
          replies[a.square] = null;
        } else {
          const child = solve(a.board, 2, depth - 1, path);
          if (!child) return null;
          replies[a.square] = child;
          rank = Math.max(rank, 1 + child.rank);
        }
      }
      return moves.length ? { replies, rank } : null;
    } finally {
      path.delete(key);
    }
  }
  for (let depth = 1; depth <= 11; depth += 2) {
    try {
      const tree = solve(board, 2, depth, prior);
      if (tree) return tree;
    } catch (e) {
      if (e !== STOP) throw e;
      return null;
    }
  }
  return null;
}
// Recheck all required replies without calling the search or trusting its scores.
export function verifyTree(board, turn, tree, history, deadline = Infinity) {
  const path = new Set(history);
  path.delete(canonical(board, turn).key);
  let nodes = 0;
  function check(board, turn, node, parentRank) {
    if (
      ++nodes > 10000 ||
      performance.now() >= deadline ||
      !node ||
      !Number.isInteger(node.rank) ||
      node.rank < 1 ||
      node.rank >= parentRank
    )
      throw Error('Invalid or oversized tactical proof');
    const key = canonical(board, turn).key;
    if (path.has(key)) throw Error('Tactical proof repeats a position');
    path.add(key);
    try {
      const moves = turn === 2 ? [node.move] : legal(board);
      if (turn === 2 && !legal(board).includes(node.move))
        throw Error('Illegal proof move');
      for (const square of moves) {
        const a = apply(board, turn, square),
          child = turn === 2 ? node.child : node.replies?.[square];
        if (a.result) {
          if (a.result.winner !== 2)
            throw Error('Tactical proof reaches a non-win');
        } else check(a.board, 3 - turn, child, node.rank);
      }
    } finally {
      path.delete(key);
    }
  }
  check(board, turn, tree, Infinity);
  return true;
}
export function analyze(board, history, budget = 3000, retained = null) {
  const start = performance.now();
  if (retained) {
    verifyTree(board, 2, retained, history, start + budget);
    return { square: retained.move, proof: retained, depth: 0 };
  }
  const fallback = bestEffort(board, history, start + budget / 3);
  const proof = forcingTree(board, history, start + budget * 0.9);
  if (proof) {
    try {
      verifyTree(board, 2, proof, history, start + budget);
      return { square: proof.move, proof, depth: fallback.depth };
    } catch {
      /* A partial or unchecked tree cannot be presented as a proof. */
    }
  }
  return { ...fallback, proof: null };
}
