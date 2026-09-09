require('./evidence.cjs');
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
process.chdir(__dirname);
const base = process.argv[2] || 'http://127.0.0.1:8769';
(async () => {
  const browser = await chromium.launch(require('./browser-options.cjs'));
  try {
    const page = await browser.newPage({
        colorScheme: 'dark',
        viewport: { width: 1100, height: 1100 },
      }),
      errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.goto(base);
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
      undefined,
      { timeout: 120000 },
    );
    // Computer opening is visibly delayed, and nothing is playable during it.
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.waitForTimeout(350);
    assert.equal(await page.locator('.motion-layer').count(), 0);
    assert.equal(await page.locator('.piece').count(), 0);
    assert.equal(await page.locator('.cell:not(:disabled)').count(), 0);
    await page.waitForSelector('.motion-land');
    assert.equal(await page.locator('#history').textContent(), 'No moves yet.');
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
    );
    await page.selectOption('#mode', 'demo');
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#board').getAttribute('aria-busy') === 'false',
    );
    // D3 pushes X from C3 to B3. The source waits for the landing, then slides.
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    const start = await page.locator('.motion-push').boundingBox();
    assert.equal(await page.locator('.motion-land').textContent(), 'O');
    assert.equal(await page.locator('#demo-step').isDisabled(), true);
    assert.equal(await page.locator('#history').textContent(), '1. X C3');
    await page.waitForFunction(
      () => document.querySelector('#board').dataset.animation === 'push',
    );
    await page.waitForTimeout(180);
    const moving = await page.locator('.motion-push').boundingBox();
    assert(
      moving.x < start.x - 5,
      'pushed piece must move left during the keyframe',
    );
    assert(Math.abs(moving.y - start.y) < 1);
    await page.screenshot({
      path: '../evidence/static-push-animation.png',
      animations: 'allow',
      fullPage: true,
    });
    await page.waitForFunction(
      () =>
        document.querySelector('#board').getAttribute('aria-busy') === 'false',
    );
    assert.equal(
      await page.getByRole('button', { name: 'B3, X', exact: true }).count(),
      1,
    );
    assert.equal(await page.locator('.motion-layer').count(), 0);
    // Cancelling a move must not commit its delayed board or leave ghosts.
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.waitForSelector('.motion-layer');
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.waitForTimeout(1400);
    assert.equal(await page.locator('.piece').count(), 0);
    assert.equal(await page.locator('#history').textContent(), 'No moves yet.');
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.waitForSelector('.motion-layer');
    await page.selectOption('#mode', 'play-first');
    await page.waitForTimeout(1400);
    assert.equal(await page.locator('.piece').count(), 0);
    assert.equal(await page.locator('.cell:not(:disabled)').count(), 36);
    // The original demo contains captures at the edge. Check fading exit keyframes too.
    await page.selectOption('#mode', 'demo');
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    let sawExit = false;
    for (let i = 0; i < 25 && !sawExit; i++) {
      await page.getByRole('button', { name: 'Step', exact: true }).click();
      if (await page.locator('.motion-exit').count()) {
        await page.waitForFunction(
          () => document.querySelector('#board').dataset.animation === 'push',
        );
        await page.waitForTimeout(380);
        assert(
          await page
            .locator('.motion-exit')
            .first()
            .evaluate((e) => Number(getComputedStyle(e).opacity) < 1),
        );
        sawExit = true;
      }
      await page.waitForFunction(
        () =>
          document.querySelector('#board').getAttribute('aria-busy') ===
          'false',
      );
    }
    assert(sawExit, 'demo must exercise a piece leaving the board');
    // Pausing during a move lets that move settle and does not schedule another.
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.getByRole('button', { name: 'Resume', exact: true }).click();
    await page.waitForSelector('.motion-land');
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#board').getAttribute('aria-busy') === 'false',
    );
    const stopped = await page.locator('#history').textContent();
    await page.waitForTimeout(1300);
    assert.equal(await page.locator('#history').textContent(), stopped);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    assert.equal(await page.locator('.motion-layer').count(), 0);
    await page.waitForFunction(
      () =>
        document.querySelector('#board').getAttribute('aria-busy') === 'false',
    );
    assert.equal(await page.locator('#history').textContent(), '1. X C3');
    assert.deepEqual(errors, []);
    console.log(
      'Motion checks passed: computer pacing, landing then real push keyframes, off-board fade, input lock, delayed commits, reset/mode cancellation, pause while moving, reduced motion.',
    );
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
