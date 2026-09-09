require('./evidence.cjs');
// Run only against a public instance set up for visitor sessions.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
process.chdir(__dirname);
const base = process.argv[2];
if (!base || new URL(base).origin !== base || !base.startsWith('https://'))
  throw Error('Pass the public HTTPS origin');
(async () => {
  const browser = await chromium.launch(require('./browser-options.cjs'));
  try {
    const a = await browser.newContext({
      colorScheme: 'dark',
      viewport: { width: 1100, height: 1100 },
    });
    const b = await browser.newContext({
      colorScheme: 'dark',
      viewport: { width: 1100, height: 1100 },
    });
    const pageA = await a.newPage(),
      pageB = await b.newPage(),
      errors = [];
    for (const page of [pageA, pageB])
      page.on('pageerror', (error) => errors.push(error.message));
    await Promise.all([pageA.goto(base), pageB.goto(base)]);
    for (const page of [pageA, pageB])
      await page.waitForFunction(
        () => !document.querySelector('#mode option[value="demo"]').disabled,
        undefined,
        { timeout: 120000 },
      );
    const read = async (context) =>
      await (await context.request.get(base + '/api/state')).json();
    const originalA = await read(a),
      originalB = await read(b);
    assert.equal(originalA.visitorSession, true);
    assert.equal(originalB.visitorSession, true);
    const ca = (await a.cookies()).find(
      (c) => c.name === '__Host-gekitai_session',
    );
    const cb = (await b.cookies()).find(
      (c) => c.name === '__Host-gekitai_session',
    );
    assert.ok(ca.secure && ca.httpOnly);
    assert.notEqual(ca.value, cb.value);
    await pageA.selectOption('#mode', 'play-first');
    await pageA.waitForFunction(
      () => document.querySelectorAll('.cell:not(:disabled)').length === 36,
    );
    await pageA.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await pageA.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Computer is thinking…',
    );
    assert.deepEqual((await read(b)).moves, originalB.moves);
    await pageA.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
    );
    assert.equal((await read(a)).moves.length, 2);
    assert.equal((await read(b)).mode, 'verified');
    const rejected = await b.request.post(base + '/api/reset', {
      headers: { Origin: 'https://unrelated.example' },
      data: { revision: originalB.revision, mode: 'play-first' },
    });
    // The tunnel can translate an origin rejection into an upstream error;
    // the security contract is rejection with the visitor's game unchanged.
    assert.equal(rejected.ok(), false);
    assert.equal((await read(b)).mode, 'verified');
    await pageB.selectOption('#mode', 'demo');
    await pageB.getByRole('button', { name: 'Pause', exact: true }).click();
    await pageB.getByRole('button', { name: 'New game', exact: true }).click();
    await pageB.waitForFunction(() =>
      document.querySelector('#bound').textContent.startsWith('0 / 25'),
    );
    for (let i = 0; i < 25; i++)
      await pageB.getByRole('button', { name: 'Step', exact: true }).click();
    await pageB.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'First player (X) wins',
    );
    assert.equal((await read(b)).moves.length, 25);
    assert.equal((await read(a)).moves.length, 2);
    await pageB.screenshot({
      path: '../evidence/public-demo.png',
      fullPage: true,
    });
    await pageA.setViewportSize({ width: 390, height: 844 });
    assert.equal(
      await pageA.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
      true,
    );
    await pageA.screenshot({
      path: '../evidence/public-practice-mobile.png',
      fullPage: true,
    });
    assert.deepEqual(errors, []);
    console.log(
      'Public HTTPS checks passed: independent visitor games, secure cookies, practice reply, origin rejection, exact 25-placement demo, mobile layout.',
    );
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
