/**
 * RustyCAN web dashboard — Playwright test suite (Phase 3).
 *
 * Architecture
 * ────────────
 * Each describe block starts a Node.js HTTP server (startMockServer) that:
 *   1. Serves the real `host/assets/index.html` verbatim.
 *   2. Exposes `GET /events` as an SSE endpoint.
 *   3. Lets tests push synthetic JSON events with `mock.inject(event)`.
 *   4. Supports `mock.setEventsMode('hang'|'normal')` to simulate drops and
 *      recovery without touching any live CAN hardware or Rust binary.
 *
 * Running
 * ───────
 *   cd tests/ui/playwright
 *   npm install && npx playwright install chromium
 *   npx playwright test
 */

import { test, expect } from '@playwright/test';
import { startMockServer, type MockServer } from '../mock-server';

// ─── Helper ──────────────────────────────────────────────────────────────────

/** Build an SSE event payload with a current timestamp. */
function ev(overrides: Record<string, unknown>): object {
  return { ts: new Date().toISOString(), ...overrides };
}

// ─── Connection status badge ──────────────────────────────────────────────────

test.describe('Connection status badge', () => {
  let mock: MockServer;

  test.beforeEach(async () => { mock = await startMockServer(); });
  test.afterEach(async ()  => { await mock.close(); });

  test('shows "Live" (green) when SSE stream is open', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');
    await expect(page.locator('#conn-dot')).toHaveClass(/connected/);
  });

  test('shows "Reconnecting…" (yellow) when SSE stream drops', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    // Close active SSE streams and hang future /events requests so the
    // browser stays in CONNECTING state (badge: "Reconnecting…").
    mock.setEventsMode('hang');

    await expect(page.locator('#conn-label')).toHaveText('Reconnecting…', { timeout: 5000 });
    await expect(page.locator('#conn-dot')).toHaveClass(/reconnecting/);
  });

  test('recovers to "Live" after SSE reconnect', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    // Drop connection and block retries.
    mock.setEventsMode('hang');
    await expect(page.locator('#conn-label')).toHaveText('Reconnecting…', { timeout: 5000 });

    // Re-enable normal SSE: hanging request gets a 503, browser retries and
    // gets a real stream → Live.
    mock.setEventsMode('normal');
    await expect(page.locator('#conn-label')).toHaveText('Live', { timeout: 10000 });
  });
});

// ─── NMT node grid ────────────────────────────────────────────────────────────

test.describe('NMT node grid', () => {
  let mock: MockServer;

  test.beforeEach(async () => { mock = await startMockServer(); });
  test.afterEach(async ()  => { await mock.close(); });

  test('renders a card for an injected Operational node', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({ type: 'NMT_STATE', node: 1, state: 'Operational' }));

    await expect(page.locator('#node-1')).toBeVisible();
    await expect(page.locator('#node-1 .node-state')).toHaveText('Operational');
    await expect(page.locator('#node-1 .node-state')).toHaveClass(/state-op/);
  });

  test('renders correct colours for Pre-Op, Stopped, Bootup states', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({ type: 'NMT_STATE', node: 1, state: 'Operational' }));
    mock.inject(ev({ type: 'NMT_STATE', node: 2, state: 'Pre-Operational' }));
    mock.inject(ev({ type: 'NMT_STATE', node: 3, state: 'Stopped' }));
    mock.inject(ev({ type: 'NMT_STATE', node: 4, state: 'Bootup' }));

    await expect(page.locator('#node-1 .node-state')).toHaveClass(/state-op/);
    await expect(page.locator('#node-2 .node-state')).toHaveClass(/state-preop/);
    await expect(page.locator('#node-3 .node-state')).toHaveClass(/state-stop/);
    await expect(page.locator('#node-4 .node-state')).toHaveClass(/state-boot/);
  });

  test('shows node label and "just now" timestamp', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({ type: 'NMT_STATE', node: 5, state: 'Operational' }));

    await expect(page.locator('#node-5 .node-label')).toHaveText('Node 5');
    await expect(page.locator('#node-5 .node-age')).toHaveText('just now');
  });

  test('sorts node cards by ID', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    // Inject out-of-order: 3, 1, 2 — grid must show them as 1, 2, 3.
    mock.inject(ev({ type: 'NMT_STATE', node: 3, state: 'Operational' }));
    mock.inject(ev({ type: 'NMT_STATE', node: 1, state: 'Operational' }));
    mock.inject(ev({ type: 'NMT_STATE', node: 2, state: 'Operational' }));

    await expect(page.locator('#nmt-grid .node-card')).toHaveCount(3);
    const ids = await page
      .locator('#nmt-grid .node-card')
      .evaluateAll((els: HTMLElement[]) => els.map(e => e.id));
    expect(ids).toEqual(['node-1', 'node-2', 'node-3']);
  });
});

// ─── Event log ────────────────────────────────────────────────────────────────

test.describe('Event log', () => {
  let mock: MockServer;

  test.beforeEach(async () => { mock = await startMockServer(); });
  test.afterEach(async ()  => { await mock.close(); });

  test('SDO_READ event appears with correct columns', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({
      type: 'SDO_READ', node: 1,
      index: '1000', subindex: '0', name: 'Device Type', value: 0x191,
    }));

    const row = page.locator('.log-entry[data-type="SDO_READ"]');
    await expect(row).toBeVisible();
    await expect(row.locator('.e-type')).toHaveText('SDO_READ');
    await expect(row.locator('.e-node')).toHaveText('node 1');
    await expect(row.locator('.e-detail')).toContainText('1000');
  });

  test('NMT_STATE event appears with correct type badge', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({ type: 'NMT_STATE', node: 2, state: 'Operational' }));

    const row = page.locator('.log-entry[data-type="NMT_STATE"]');
    await expect(row).toBeVisible();
    await expect(row).toHaveClass(/e-NMT_STATE/);
    await expect(row.locator('.e-type')).toHaveText('NMT_STATE');
  });

  test('log is capped at 200 rows', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    for (let i = 0; i < 210; i++) {
      mock.inject(ev({ type: 'SDO_READ', node: 1, index: String(0x1000 + i), subindex: '0' }));
    }

    // Playwright polls until the DOM has been trimmed to exactly MAX_LOG (200).
    await expect(page.locator('#event-log .log-entry')).toHaveCount(200, { timeout: 5000 });
  });
});

// ─── Filter buttons ───────────────────────────────────────────────────────────

test.describe('Filter buttons', () => {
  let mock: MockServer;

  test.beforeEach(async () => { mock = await startMockServer(); });
  test.afterEach(async ()  => { await mock.close(); });

  test('SDO filter hides NMT rows and shows SDO rows', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({ type: 'NMT_STATE', node: 1, state: 'Operational' }));
    mock.inject(ev({ type: 'SDO_READ',  node: 1, index: '1000', subindex: '0' }));

    // Both rows visible under the default "All" filter.
    await expect(page.locator('.log-entry[data-type="NMT_STATE"]')).toBeVisible();
    await expect(page.locator('.log-entry[data-type="SDO_READ"]')).toBeVisible();

    await page.locator('.filter-btn[data-type="SDO"]').click();

    await expect(page.locator('.log-entry[data-type="NMT_STATE"]')).toBeHidden();
    await expect(page.locator('.log-entry[data-type="SDO_READ"]')).toBeVisible();
  });

  test('All filter restores hidden rows', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    mock.inject(ev({ type: 'NMT_STATE', node: 1, state: 'Operational' }));
    mock.inject(ev({ type: 'SDO_READ',  node: 1, index: '1000', subindex: '0' }));

    await page.locator('.filter-btn[data-type="SDO"]').click();
    await expect(page.locator('.log-entry[data-type="NMT_STATE"]')).toBeHidden();

    await page.locator('.filter-btn[data-type="ALL"]').click();

    await expect(page.locator('.log-entry[data-type="NMT_STATE"]')).toBeVisible();
    await expect(page.locator('.log-entry[data-type="SDO_READ"]')).toBeVisible();
  });
});

// ─── Pause / resume ───────────────────────────────────────────────────────────

test.describe('Pause / resume', () => {
  let mock: MockServer;

  test.beforeEach(async () => { mock = await startMockServer(); });
  test.afterEach(async ()  => { await mock.close(); });

  test('pause button stops log updates', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    // Seed one row to confirm the log is working.
    mock.inject(ev({ type: 'SDO_READ', node: 1, index: '1000', subindex: '0' }));
    await expect(page.locator('#event-log .log-entry')).toHaveCount(1);

    await page.locator('#pause-btn').click();

    mock.inject(ev({ type: 'SDO_READ', node: 1, index: '1001', subindex: '0' }));
    mock.inject(ev({ type: 'SDO_READ', node: 1, index: '1002', subindex: '0' }));

    // Allow time for SSE events to arrive at the browser; they should be
    // buffered internally but not rendered while paused.
    await page.waitForTimeout(300);
    await expect(page.locator('#event-log .log-entry')).toHaveCount(1);
  });

  test('resume button flushes buffered events', async ({ page }) => {
    await page.goto(mock.url);
    await expect(page.locator('#conn-label')).toHaveText('Live');

    await page.locator('#pause-btn').click();

    mock.inject(ev({ type: 'SDO_READ', node: 1, index: '2000', subindex: '0' }));
    mock.inject(ev({ type: 'SDO_READ', node: 1, index: '2001', subindex: '0' }));

    // Allow delivery; rows must NOT appear while paused.
    await page.waitForTimeout(300);
    await expect(page.locator('#event-log .log-entry')).toHaveCount(0);

    // Resume — buffered events must flush to the DOM.
    await page.locator('#pause-btn').click();
    await expect(page.locator('#event-log .log-entry')).toHaveCount(2);
  });
});

// ─── Dark mode ────────────────────────────────────────────────────────────────

test.describe('Dark mode', () => {
  let mock: MockServer;

  test.beforeEach(async () => { mock = await startMockServer(); });
  test.afterEach(async ()  => { await mock.close(); });

  test('CSS variables switch when prefers-color-scheme is dark', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.goto(mock.url);

    const bg = await page.evaluate(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--bg').trim(),
    );
    expect(bg).toBe('#161617');
  });
});

