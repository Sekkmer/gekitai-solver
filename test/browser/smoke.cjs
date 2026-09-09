require('./evidence.cjs');
// Run against the prepared-position fixture, never the production game.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
process.chdir(__dirname);
(async () => {
  const browser = await chromium.launch(require('./browser-options.cjs'));
  try {
    const page = await browser.newPage({
      colorScheme: 'light',
      viewport: { width: 1100, height: 1050 },
    });
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    const initial = await (
      await page.request.get('http://127.0.0.1:8766/api/state')
    ).json();
    await page.request.post('http://127.0.0.1:8766/api/reset', {
      headers: { Origin: 'http://127.0.0.1:8766' },
      data: { revision: initial.revision, mode: 'verified' },
    });
    await page.goto('http://127.0.0.1:8766');
    await page.waitForFunction(
      () =>
        document.querySelector('#badge').textContent === 'Strategy verified',
    );
    assert.equal(await page.locator('#start-note').isVisible(), true);
    await page.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Second player (O) wins',
    );
    assert.equal(await page.locator('.cell:not(:disabled)').count(), 0);
    assert.match(await page.locator('#history').textContent(), /1\. X C3/);
    await page.screenshot({
      path: '../evidence/player-game-over.png',
      fullPage: true,
    });
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
    );
    assert.equal(await page.locator('.cell:not(:disabled)').count(), 32);
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
      true,
    );
    await page.screenshot({
      path: '../evidence/player-mobile.png',
      fullPage: true,
    });
    await page
      .getByRole('button', { name: 'Switch to dark mode', exact: true })
      .click();
    assert.equal(
      await page.evaluate(() => document.documentElement.dataset.theme),
      'dark',
    );
    await page.screenshot({
      path: '../evidence/player-mobile-dark.png',
      fullPage: true,
    });
    await page.reload();
    await page.waitForFunction(
      () =>
        document.querySelector('#badge').textContent === 'Strategy verified',
    );
    assert.equal(
      await page.evaluate(() => document.documentElement.dataset.theme),
      'dark',
    );
    await page.setViewportSize({ width: 1100, height: 1050 });
    await page.screenshot({
      path: '../evidence/player-dark.png',
      fullPage: true,
    });
    await page
      .getByRole('button', { name: 'Switch to light mode', exact: true })
      .click();
    assert.equal(
      await page.evaluate(() => document.documentElement.dataset.theme),
      'light',
    );
    const darkContext = await browser.newContext({ colorScheme: 'dark' });
    const darkPage = await darkContext.newPage();
    await darkPage.goto('http://127.0.0.1:8766');
    assert.equal(
      await darkPage.evaluate(() => document.documentElement.dataset.theme),
      'dark',
    );
    await darkContext.close();
    await page.selectOption('#mode', 'play-first');
    await page.waitForFunction(
      () => document.querySelectorAll('.cell:not(:disabled)').length === 36,
    );
    await page.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Computer is thinking…',
    );
    const responseStarted = Date.now();
    const pending = await (
      await page.request.get('http://127.0.0.1:8766/api/state')
    ).json();
    assert.equal(pending.thinking, true);
    assert.ok(
      Date.now() - responseStarted < 800,
      'State reads must remain responsive while thinking',
    );
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.waitForTimeout(3400);
    assert.equal(await page.locator('#history').textContent(), 'No moves yet.');
    const moveStarted = Date.now();
    await page.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Computer is thinking…',
    );
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
    );
    assert.ok(
      Date.now() - moveStarted < 5500,
      'Practice should reply in roughly three seconds',
    );
    assert.match(await page.locator('#history').textContent(), /2\. O/);
    assert.match(
      await page.locator('#bound').textContent(),
      /No forced win has been verified/,
    );
    await page.screenshot({
      path: '../evidence/player-practice.png',
      fullPage: true,
    });
    await page.selectOption('#mode', 'demo');
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    const pausedHistory = await page.locator('#history').textContent();
    await page.waitForTimeout(1600);
    assert.equal(await page.locator('#history').textContent(), pausedHistory);
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Second player (O) wins',
    );
    assert.match(
      await page.locator('#bound').textContent(),
      /2 \/ 2 placements/,
    );
    assert.equal(
      await page
        .getByRole('button', { name: 'Step', exact: true })
        .isDisabled(),
      true,
    );
    await page.screenshot({
      path: '../evidence/player-demo.png',
      fullPage: true,
    });
    await page.selectOption('#mode', 'verified');
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
    );
    assert.deepEqual(errors, []);
    console.log(
      'Browser checks passed: placement, automatic verified reply, terminal lock, reset, mobile width, dark/light toggle, saved theme, system theme, practice timing, cancellation, demo pause/step, no JavaScript errors.',
    );
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
