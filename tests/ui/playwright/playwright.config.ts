import { defineConfig } from '@playwright/test';

/**
 * Playwright configuration for RustyCAN web dashboard tests.
 *
 * Tests use a lightweight mock SSE server (started per-test in the spec file)
 * that serves the real `host/assets/index.html` and pushes synthetic events —
 * no live RustyCAN process or CAN hardware is required.
 *
 * Only Chromium is used: the dashboard is a simple SSE + DOM page with no
 * browser-specific APIs, so one engine is sufficient for CI coverage across
 * all three platforms (macOS, Windows, Linux).
 */
export default defineConfig({
  testDir: './tests',
  timeout: 15_000,
  retries: 1,
  use: {
    browserName: 'chromium',
    headless: true,
    // Capture screenshots on failure for debugging.
    screenshot: 'only-on-failure',
    // Capture traces on first retry.
    trace: 'on-first-retry',
  },
  // Store test artifacts (screenshots, traces, reports) outside the src tree.
  outputDir: '../../../target/playwright-results',
  reporter: [
    ['list'],
    ['html', { outputFolder: '../../../target/playwright-report', open: 'never' }],
  ],
});
