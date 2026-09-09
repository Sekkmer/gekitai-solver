require('./evidence.cjs');
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
process.chdir(__dirname);
const base = process.argv[2] || 'http://127.0.0.1:8769';
(async () => {
  const browser = await chromium.launch(require('./browser-options.cjs'));
  try {
    const context = await browser.newContext({
      colorScheme: 'dark',
      viewport: { width: 1100, height: 1100 },
    });
    const page = await context.newPage(),
      errors = [],
      requests = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('request', (r) => requests.push(r.url()));
    await page.goto(base);
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
      undefined,
      { timeout: 120000 },
    );
    assert.equal(await page.locator('.piece.x').count(), 1);
    await page.getByRole('button', { name: 'A1, empty', exact: true }).click();
    await page.waitForFunction(() =>
      document
        .querySelector('#history')
        .textContent.startsWith('1. X C3  ·  2. O A1  ·  3. X'),
    );
    // Every tab owns its board; no API, session server, or cookies.
    const second = await context.newPage();
    await second.goto(base);
    await second.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
    );
    assert.equal(await second.locator('#history').textContent(), '1. X C3');
    await second.close();
    await page.selectOption('#mode', 'play-first');
    assert.equal(await page.locator('.cell:not(:disabled)').count(), 36);
    await page.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Computer is thinking…',
    );
    // Theme remains responsive during the worker search.
    await page
      .getByRole('button', { name: 'Switch to light mode', exact: true })
      .click();
    await page.waitForFunction(
      () => document.documentElement.dataset.theme === 'light',
    );
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
      undefined,
      { timeout: 10000 },
    );
    assert.match(
      await page.locator('#history').textContent(),
      /^1\. X C3  ·  2\. O /,
    );
    await page
      .getByRole('button', { name: 'Switch to dark mode', exact: true })
      .click();
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
    );
    await page.screenshot({
      path: '../evidence/static-practice-mobile.png',
      fullPage: true,
    });
    // Cancel in flight and verify no late answer is applied.
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'Computer is thinking…',
    );
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.waitForTimeout(3300);
    assert.equal(await page.locator('.piece').count(), 0);
    assert.equal(await page.locator('.cell:not(:disabled)').count(), 36);
    await page.selectOption('#mode', 'demo');
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    for (let i = 0; i < 25; i++)
      await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#board').getAttribute('aria-busy') === 'false',
    );
    assert.equal(
      await page.locator('#status').textContent(),
      'First player (X) wins',
    );
    assert.equal(
      await page.locator('#detail').textContent(),
      'All eight pieces are on the board.',
    );
    assert.equal(await page.locator('.piece.x').count(), 8);
    await page.setViewportSize({ width: 1100, height: 1100 });
    await page.screenshot({
      path: '../evidence/static-demo.png',
      fullPage: true,
    });
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.getByRole('button', { name: 'Resume', exact: true }).click();
    await page.waitForFunction(() =>
      document.querySelector('#history').textContent.startsWith('1. X C3'),
    );
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    const history = await page.locator('#history').textContent();
    await page.waitForTimeout(1600);
    assert.equal(await page.locator('#history').textContent(), history);
    // Practice works while the strategy download is blocked, and a late load never resets it.
    const delayed = await browser.newContext();
    await delayed.route('**/assets/strategy-*.bin.gz', (route) =>
      route.abort(),
    );
    const p = await delayed.newPage();
    p.on('pageerror', (e) => errors.push(e.message));
    await p.goto(base);
    await p.selectOption('#mode', 'play-first');
    await p.getByRole('button', { name: 'C3, empty', exact: true }).click();
    await p.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
      undefined,
      { timeout: 10000 },
    );
    assert.match(
      await p.locator('#history').textContent(),
      /^1\. X C3  ·  2\. O /,
    );
    assert.equal(await p.locator('#error').textContent(), '');
    await delayed.close();
    // The cached strategy works on reload without transferring the asset again.
    await page.route('**/assets/strategy-*.bin.gz', (route) => route.abort());
    await page.reload();
    await page.waitForFunction(
      () => document.querySelector('#status').textContent === 'Your move.',
      undefined,
      { timeout: 30000 },
    );
    assert.equal(await page.locator('.piece.x').count(), 1);
    assert(!requests.some((url) => url.includes('/api/')));
    assert.deepEqual(errors, []);
    console.log(
      'Static browser checks passed: verified play, separate tabs, practice search and cancellation, responsive theme, 25-move demo and win reason, pause/resume, mobile, failed download isolation, cached strategy, no API calls.',
    );
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
