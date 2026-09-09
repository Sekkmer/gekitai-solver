require('./evidence.cjs');
// Requires the full clean-first certificate on the preview server, port 8767.
// Kept separate from smoke.cjs, which only needs the tiny prepared fixture.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
process.chdir(__dirname);
(async () => {
  const browser = await chromium.launch(require('./browser-options.cjs'));
  try {
    const page = await browser.newPage({
      colorScheme: 'dark',
      viewport: { width: 1100, height: 1080 },
    });
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    const initial = await (
      await page.request.get('http://127.0.0.1:8767/api/state')
    ).json();
    assert.equal(initial.certificateReady, true);
    await page.request.post('http://127.0.0.1:8767/api/reset', {
      headers: { Origin: 'http://127.0.0.1:8767' },
      data: { revision: initial.revision, mode: 'verified' },
    });
    await page.goto('http://127.0.0.1:8767');
    await page.waitForFunction(
      () =>
        document.querySelector('#badge').textContent === 'Strategy verified',
    );
    await page.selectOption('#mode', 'demo');
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    assert.match(
      await page.locator('#bound').textContent(),
      /0 \/ 25 placements/,
    );
    for (let i = 0; i < 25; i++)
      await page.getByRole('button', { name: 'Step', exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector('#status').textContent ===
        'First player (X) wins',
    );
    assert.match(
      await page.locator('#bound').textContent(),
      /25 \/ 25 placements/,
    );
    await page.screenshot({
      path: '../evidence/player-demo-25-dark.png',
      fullPage: true,
    });
    // Restart, then verify automatic playback stops and resumes on demand.
    await page.getByRole('button', { name: 'New game', exact: true }).click();
    await page.getByRole('button', { name: 'Resume', exact: true }).click();
    await page.waitForFunction(() =>
      document.querySelector('#history').textContent.includes('2. O'),
    );
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    const history = await page.locator('#history').textContent();
    await page.waitForTimeout(1700);
    assert.equal(await page.locator('#history').textContent(), history);
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
      true,
    );
    await page.screenshot({
      path: '../evidence/player-demo-mobile.png',
      fullPage: true,
    });
    assert.deepEqual(errors, []);
    console.log(
      'Full certificate demo passed: exactly 25 placements, first-player win, autoplay, pause/resume, step controls, mobile layout.',
    );
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
