/**
 * Lightweight mock HTTP + SSE server for RustyCAN dashboard tests.
 *
 * Each test creates its own instance (via `startMockServer()`) on a random
 * port, so tests can run in parallel without port conflicts.
 *
 * Capabilities
 * ────────────
 *  • Serves `host/assets/index.html` verbatim for every GET request that is
 *    not a recognised API path.
 *  • `GET /events`  — SSE endpoint; keeps connections open and streams JSON
 *                     events injected by `mock.inject(event)`.
 *  • `GET /logo.png`— returns a 1 × 1 transparent PNG so the browser console
 *                     stays quiet (no 404 noise).
 *  • `mock.inject(event)` — pushes a JSON object to every connected SSE client.
 *  • `mock.setEventsMode('hang')` — closes current SSE streams (triggers
 *    EventSource `onerror` in the browser) and makes subsequent `/events`
 *    requests hang without a response, keeping the UI badge in "Reconnecting…".
 *  • `mock.setEventsMode('normal')` — re-enables proper SSE responses.
 *    Hanging requests receive a 503 so the browser retries and reconnects.
 *  • `mock.close()` — gracefully shuts down the server.
 */

import { createServer, IncomingMessage, ServerResponse } from 'node:http';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { AddressInfo } from 'node:net';

// Path to the real dashboard HTML — served verbatim so tests exercise
// the actual production asset.
const HTML_PATH = join(__dirname, '../../../host/assets/index.html');

// Path to the MesloLGS NF font — served so the @font-face CSS rule resolves
// correctly and doesn't trigger browser console errors during tests.
const FONT_PATH = join(__dirname, '../../../host/assets/MesloLGSNF-Regular.ttf');

// Minimal 1 × 1 transparent PNG; avoids 404 noise for /logo.png.
const LOGO_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=',
  'base64',
);

export interface MockServer {
  /** Base URL of the local server, e.g. `http://127.0.0.1:54321`. */
  url: string;

  /** Push a JSON event into every currently-connected SSE stream. */
  inject(event: object): void;

  /**
   * Control how `/events` requests are handled.
   *
   * - `'normal'`  Current SSE streams keep running; future requests get a
   *               proper `text/event-stream` response.
   * - `'hang'`    Close all active SSE streams (browser fires `onerror`).
   *               Future `/events` requests are accepted at TCP level but
   *               receive no HTTP response, so EventSource stays CONNECTING
   *               and the UI badge shows "Reconnecting…".
   *               Switching back to `'normal'` closes hanging requests with
   *               a 503 so the browser retries successfully.
   */
  setEventsMode(mode: 'normal' | 'hang'): void;

  /** Gracefully shut down the server and end all open connections. */
  close(): Promise<void>;
}

export async function startMockServer(): Promise<MockServer> {
  const sseClients: ServerResponse[]  = [];   // active SSE connections
  const hangingReqs: ServerResponse[] = [];   // connections held open but unanswered

  let eventsMode: 'normal' | 'hang' = 'normal';

  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    if (req.method === 'GET' && req.url === '/events') {
      if (eventsMode === 'hang') {
        // Accept the TCP connection but never write a response.
        // The browser's EventSource stays in CONNECTING state →
        // setConn('reconnecting') is already set from the connect() call.
        hangingReqs.push(res);
        req.on('close', () => {
          const i = hangingReqs.indexOf(res);
          if (i >= 0) hangingReqs.splice(i, 1);
        });
        return;
      }

      res.writeHead(200, {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache',
        'Connection': 'keep-alive',
      });
      res.write(': ping\n\n'); // initial flush triggers EventSource.onopen

      sseClients.push(res);
      req.on('close', () => {
        const i = sseClients.indexOf(res);
        if (i >= 0) sseClients.splice(i, 1);
      });

    } else if (req.method === 'GET' && req.url === '/logo.png') {
      res.writeHead(200, { 'Content-Type': 'image/png' });
      res.end(LOGO_PNG);

    } else if (req.method === 'GET' && req.url === '/font/meslo.ttf') {
      try {
        const font = readFileSync(FONT_PATH);
        res.writeHead(200, { 'Content-Type': 'font/ttf' });
        res.end(font);
      } catch {
        res.writeHead(404);
        res.end();
      }

    } else {
      // Serve the real dashboard HTML for all other GET requests.
      try {
        const html = readFileSync(HTML_PATH, 'utf-8');
        res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
        res.end(html);
      } catch {
        res.writeHead(500);
        res.end('Could not read host/assets/index.html');
      }
    }
  });

  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', () => resolve()));
  const { port } = server.address() as AddressInfo;
  const url = `http://127.0.0.1:${port}`;

  return {
    url,

    inject(event: object) {
      const data = `data: ${JSON.stringify(event)}\n\n`;
      for (const c of sseClients) c.write(data);
    },

    setEventsMode(mode: 'normal' | 'hang') {
      eventsMode = mode;

      if (mode === 'hang') {
        // Close active SSE streams cleanly → browser fires onerror.
        for (const c of [...sseClients]) c.end();
        sseClients.length = 0;
      } else {
        // Send 503 to hanging requests → browser fires onerror, then
        // retries and this time gets a real SSE stream.
        for (const c of [...hangingReqs]) { c.writeHead(503); c.end(); }
        hangingReqs.length = 0;
      }
    },

    close(): Promise<void> {
      return new Promise(resolve => {
        for (const c of sseClients)  c.end();
        for (const c of hangingReqs) { c.writeHead(503); c.end(); }
        // closeAllConnections is available from Node 18.2+.
        (server as { closeAllConnections?: () => void }).closeAllConnections?.();
        server.close(() => resolve());
      });
    },
  };
}
