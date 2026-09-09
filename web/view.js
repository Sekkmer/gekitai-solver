import { resultText } from './engine.js';
export const $ = (id) => document.getElementById(id);
export const coord = (i) => 'ABCDEF'[i % 6] + (Math.floor(i / 6) + 1);

/** DOM rendering only: the controller owns game state and worker lifetimes. */
export function createView(onMove) {
  const cells = [];
  for (let i = 0; i < 36; i++) {
    const b = document.createElement('button');
    b.className = 'cell';
    b.onclick = () => onMove(i);
    $('board').append(b);
    cells.push(b);
  }
  function render({
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
  }) {
    const demo = mode === 'demo',
      practice = mode === 'play-first',
      waiting = mode === 'verified' && !ready;
    $('mode').value = mode;
    $('reset').disabled = false;
    $('badge').textContent = waiting
      ? 'Loading strategy'
      : thinking
        ? 'Thinking…'
        : practice
          ? proofBound
            ? 'Forced win verified'
            : 'Practice · 3 seconds'
          : 'Strategy verified';
    $('turn').textContent = demo
      ? 'Computer vs computer'
      : practice
        ? 'You play X · First'
        : 'You play O · Second';
    $('status').textContent = result
      ? resultText(result)
      : animating
        ? movingText
        : waiting
          ? 'Preparing the strategy…'
          : thinking
            ? 'Computer is thinking…'
            : demo
              ? demoPlaying
                ? 'Watch the game.'
                : 'Demo paused.'
              : 'Your move.';
    $('detail').textContent = result
      ? result.reason + '.'
      : animating
        ? 'The piece lands, then unblocked neighbours move outward.'
        : waiting
          ? loadMessage
          : practice
            ? thinking
              ? 'Searching for a strong reply on your device. Usually about three seconds.'
              : 'You move first. The computer looks for winning replies after each move.'
            : demo
              ? 'Watch a checked 25-placement game. Pause or step through each move.'
              : 'Choose an empty square. The computer responds with a move from the verified strategy.';
    $('bound').textContent = waiting
      ? 'Practice and the demo are ready to play now.'
      : demo
        ? `${moves.length} / 25 placements · This line reaches the full certified bound.`
        : practice
          ? proofBound
            ? `A forced win was verified after placement ${proofAt}. At most ${remaining} placements remain.`
            : 'A three-second search can miss mistakes; ordinary moves are not guaranteed perfect.'
          : `Computer forces a clean win within 25 total placements.${result ? '' : ` At most ${25 - moves.length} remain.`}`;
    $('x').textContent =
      `X · ${demo || !practice ? 'Computer' : 'You'} · ${8 - board.filter((v) => v === 1).length} in supply`;
    $('o').textContent =
      `O · ${demo || practice ? 'Computer' : 'You'} · ${8 - board.filter((v) => v === 2).length} in supply`;
    $('history').textContent = moves.length
      ? moves
          .map((m, i) => `${i + 1}. ${i % 2 ? 'O' : 'X'} ${coord(m)}`)
          .join('  ·  ')
      : 'No moves yet.';
    $('error').textContent = error;
    $('demo-controls').hidden = !demo;
    $('demo-pause').textContent = demoPlaying ? 'Pause' : 'Resume';
    $('demo-pause').disabled = !!result;
    $('demo-step').disabled = !!result || animating;
    $('board').setAttribute('aria-busy', String(animating || thinking));
    const human =
      !demo &&
      !waiting &&
      !thinking &&
      !animating &&
      !result &&
      !error &&
      turn === (practice ? 1 : 2);
    for (let i = 0; i < 36; i++) {
      const b = cells[i],
        v = board[i];
      b.replaceChildren();
      if (v) {
        const p = document.createElement('span');
        p.className = 'piece ' + (v === 1 ? 'x' : 'o');
        p.textContent = v === 1 ? 'X' : 'O';
        b.append(p);
      }
      b.disabled = !human || v !== 0;
      b.classList.toggle('last', moves.at(-1) === i);
      b.classList.toggle('winning', !!result?.squares?.includes(i));
      b.setAttribute(
        'aria-label',
        coord(i) + ', ' + (v === 1 ? 'X' : v === 2 ? 'O' : 'empty'),
      );
    }
  }

  return { cells, render };
}
