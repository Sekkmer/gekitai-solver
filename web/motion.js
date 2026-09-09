// Timing is shared by the CSS keyframes and the turn sequencer.
const LAND = 300;
const PUSH_AT = 450;
const PUSH = 600;
const SETTLE = 220;
export const COMPUTER_PAUSE = 850;
export const DEMO_PAUSE = 1000;

export function pause(ms, signal) {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve(false);
    const finish = (completed) => {
      clearTimeout(timer);
      signal.removeEventListener('abort', cancel);
      resolve(completed);
    };
    const cancel = () => finish(false);
    const timer = setTimeout(() => finish(true), ms);
    signal.addEventListener('abort', cancel, { once: true });
  });
}

export async function animateMove({
  element,
  cells,
  before,
  owner,
  square,
  trace,
  signal,
}) {
  if (signal.aborted) return false;
  // Keep a short visual beat without spatial movement for reduced-motion users.
  if (matchMedia('(prefers-reduced-motion: reduce)').matches)
    return pause(120, signal);
  const bounds = element.getBoundingClientRect();
  const size = cells[0].getBoundingClientRect().width * 0.69;
  const pitch =
    cells[1].getBoundingClientRect().left -
    cells[0].getBoundingClientRect().left;
  const layer = document.createElement('div');
  layer.className = 'motion-layer';
  layer.setAttribute('aria-hidden', 'true');
  layer.style.setProperty('--land-time', `${LAND}ms`);
  layer.style.setProperty('--push-at', `${PUSH_AT}ms`);
  layer.style.setProperty('--push-time', `${PUSH}ms`);
  const pushed = new Map(trace.map((move) => [move.from, move]));
  for (let i = 0; i < 36; i++) {
    const value = i === square ? owner : before[i];
    if (!value) continue;
    const rect = cells[i].getBoundingClientRect();
    const token = document.createElement('span');
    token.className = 'motion-token';
    token.dataset.square = i;
    token.style.left = `${((rect.left + rect.width / 2 - bounds.left) / bounds.width) * 100}%`;
    token.style.top = `${((rect.top + rect.height / 2 - bounds.top) / bounds.height) * 100}%`;
    token.style.width = `${(size / bounds.width) * 100}%`;
    const piece = document.createElement('span');
    piece.className = 'piece ' + (value === 1 ? 'x' : 'o');
    piece.textContent = value === 1 ? 'X' : 'O';
    token.append(piece);
    if (i === square) token.classList.add('motion-land');
    const move = pushed.get(i);
    if (move) {
      token.classList.add(move.to === null ? 'motion-exit' : 'motion-push');
      token.style.setProperty(
        '--push-x',
        `${((move.dc * pitch) / size) * 100}%`,
      );
      token.style.setProperty(
        '--push-y',
        `${((move.dr * pitch) / size) * 100}%`,
      );
    }
    layer.append(token);
  }
  element.classList.add('is-animating');
  element.dataset.animation = 'placement';
  cells[square].classList.add('placing');
  element.append(layer);
  try {
    if (!(await pause(PUSH_AT, signal))) return false;
    element.dataset.animation = trace.length ? 'push' : 'settle';
    return await pause((trace.length ? PUSH : 0) + SETTLE, signal);
  } finally {
    layer.remove();
    element.classList.remove('is-animating');
    delete element.dataset.animation;
    cells[square].classList.remove('placing');
  }
}
