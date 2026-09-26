// Unit tests for the app end-to-end helpers (pure parts only). Run: node --test scripts/app-e2e/helpers.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";

import { DEV_URL, pageKind } from "./page.mjs";
import { unexpectedConnections } from "./win.mjs";

test("the bundled page is the app page", () => {
  assert.equal(pageKind("http://tauri.localhost/"), "app");
  assert.equal(pageKind("https://tauri.localhost/index.html"), "app");
});

test("a build without custom-protocol loads the dev server and is recognised", () => {
  // A plain `cargo build` sets cfg(dev), so the window loads devUrl instead of the bundle.
  assert.equal(DEV_URL, "http://localhost:1420");
  assert.equal(pageKind("http://localhost:1420/"), "dev");
  assert.equal(pageKind("chrome-error://chromewebdata/"), "other");
  assert.equal(pageKind("about:blank"), null);
  assert.equal(pageKind(""), null);
});

test("only the DevTools socket on loopback is allowed", () => {
  const port = 9333;
  const devtools = { LocalAddress: "127.0.0.1", LocalPort: port, RemoteAddress: "127.0.0.1", RemotePort: 50000 };
  const devServer = { LocalAddress: "::1", LocalPort: 50001, RemoteAddress: "::1", RemotePort: 1420 };
  const remote = { LocalAddress: "10.0.0.2", LocalPort: 50002, RemoteAddress: "1.2.3.4", RemotePort: port };
  assert.deepEqual(unexpectedConnections([devtools, devServer, remote], port), [devServer, remote]);
});
