import { apply, canonical, DEMO } from './engine.js';
import { createView, $, coord } from './view.js';
import { initializeTheme } from './theme.js';
import { StrategyClient } from './strategy-client.js';
import { animateMove, pause, COMPUTER_PAUSE, DEMO_PAUSE } from './motion.js';
let board,
  turn,
  moves,
  result,
  seen,
  proof,
  proofBound,
  proofAt,
  remaining,
  mode = 'verified',
  thinking = false,
  generation = 0,
  demoPlaying = false;
let ready = false,
  loading = false,
  loadMessage = 'Loading the strategy · 12 MB…',
  error = '',
  practiceWorker;
let animating = false,
  movingText = '',
  lifetime = new AbortController(),
  demoTimer;
const strategyClient = new StrategyClient((progress) => {
  loadMessage = progress;
  render();
});
const strategy = (type, board) => strategyClient.request(type, board);
initializeTheme();
const { cells, render: draw } = createView(humanMove);
function render() {
  draw({
    board,
    turn,
    moves,
    result,
    proofBound,
    proofAt,
    remaining,
    mode,
    thinking,
    demoPlaying,
    ready,
    loadMessage,
    error,
    animating,
    movingText,
  });
}
async function play(square) {
  if (result || animating) throw Error('Wait for the current move to finish');
  const gen = generation,
    signal = lifetime.signal,
    trace = [];
  const a = apply(board, turn, square, trace);
  let nextProof = proof;
  if (proof) {
    if (turn === 2 && proof.move !== square)
      throw Error('Move does not match the checked continuation');
    nextProof = turn === 2 ? proof.child : proof.replies?.[square];
    if ((a.result && a.result.winner !== 2) || (!a.result && !nextProof))
      throw Error('Move is outside the checked continuation');
  }
  const nextTurn = 3 - turn,
    key = canonical(a.board, nextTurn).key;
  let nextResult = a.result;
  if (!nextResult && seen.has(key))
    nextResult = { winner: 0, reason: 'The same position repeated' };
  if (proof && nextResult?.winner === 0)
    throw Error('Checked continuation repeats a position');
  if (mode === 'verified' && nextResult && nextResult.winner !== 1)
    throw Error('Result contradicts the verified strategy');
  animating = true;
  movingText = `${turn === 1 ? 'X' : 'O'} plays ${coord(square)}.`;
  render();
  try {
    if (
      !(await animateMove({
        element: $('board'),
        cells,
        before: board,
        owner: turn,
        square,
        trace,
        signal,
      })) ||
      gen !== generation
    )
      return false;
  } finally {
    if (gen === generation) animating = false;
  }
  board = a.board;
  turn = nextTurn;
  result = nextResult;
  moves.push(square);
  seen.add(key);
  proof = nextProof;
  if (remaining !== null) remaining--;
  render();
  return true;
}
async function loadStrategy() {
  if (ready || loading) return;
  loading = true;
  try {
    await strategy('load');
    ready = true;
    if (mode === 'verified' && !moves.length) await computerMove();
  } catch (e) {
    if (mode === 'verified') error = e.message;
  } finally {
    loading = false;
    render();
  }
}
async function computerMove() {
  const gen = generation,
    signal = lifetime.signal,
    started = performance.now();
  thinking = true;
  render();
  try {
    let answer;
    if (mode === 'verified') answer = await strategy('move', board);
    else {
      practiceWorker?.terminate();
      practiceWorker = new Worker(
        new URL('./practice-worker.js', import.meta.url),
        { type: 'module' },
      );
      const worker = practiceWorker;
      answer = await new Promise((resolve, reject) => {
        worker.onmessage = ({ data }) =>
          data.error ? reject(Error(data.error)) : resolve(data);
        worker.onerror = () =>
          reject(
            Error(
              'Practice search could not finish. Start a new game to retry.',
            ),
          );
        worker.postMessage({ id: gen, board, history: [...seen], proof });
      });
      if (gen !== generation) return;
      if (answer.proof) {
        proof = answer.proof;
        if (proofBound === null) {
          proofBound = proof.rank;
          proofAt = moves.length;
          remaining = proof.rank;
        }
      }
    }
    if (gen !== generation) return;
    if (
      !(await pause(
        Math.max(0, COMPUTER_PAUSE - (performance.now() - started)),
        signal,
      )) ||
      gen !== generation
    )
      return;
    await play(answer.square);
  } catch (e) {
    if (gen === generation) error = e.message;
  } finally {
    if (gen === generation) {
      thinking = false;
      render();
    }
  }
}
async function humanMove(square) {
  if (
    animating ||
    thinking ||
    result ||
    error ||
    mode === 'demo' ||
    turn !== (mode === 'play-first' ? 1 : 2) ||
    (mode === 'verified' && !ready)
  )
    return;
  const gen = generation;
  try {
    if ((await play(square)) && !result) computerMove();
  } catch (e) {
    if (gen === generation) {
      error = e.message;
      render();
    }
  }
}
function scheduleDemo() {
  clearTimeout(demoTimer);
  if (demoPlaying && mode === 'demo' && !animating && !result && !error)
    demoTimer = setTimeout(step, DEMO_PAUSE);
}
function reset(nextMode = mode) {
  generation++;
  lifetime.abort();
  lifetime = new AbortController();
  clearTimeout(demoTimer);
  practiceWorker?.terminate();
  practiceWorker = null;
  mode = nextMode;
  thinking = false;
  animating = false;
  error = '';
  board = Array(36).fill(0);
  turn = 1;
  moves = [];
  result = null;
  proof = null;
  proofBound = null;
  proofAt = null;
  remaining = null;
  seen = new Set([canonical(board, turn).key]);
  render();
  if (mode === 'verified') {
    if (ready) computerMove();
    else loadStrategy();
  } else scheduleDemo();
}
async function step() {
  if (mode !== 'demo' || result || animating || error) return;
  const gen = generation;
  try {
    if (!(await play(DEMO[moves.length]))) return;
    if (result && (result.winner !== 1 || moves.length !== 25))
      throw Error('Unexpected demo result');
  } catch (e) {
    if (gen === generation) {
      error = e.message;
      demoPlaying = false;
    }
  } finally {
    if (gen === generation) {
      render();
      scheduleDemo();
    }
  }
}
$('mode').onchange = () => {
  demoPlaying = $('mode').value === 'demo';
  reset($('mode').value);
};
$('reset').onclick = () => reset();
$('demo-pause').onclick = () => {
  demoPlaying = !demoPlaying;
  scheduleDemo();
  render();
};
$('demo-step').onclick = () => {
  demoPlaying = false;
  clearTimeout(demoTimer);
  step();
};
reset();
