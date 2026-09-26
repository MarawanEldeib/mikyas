// Which WebView2 page the app end-to-end test is looking at.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const conf = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "src-tauri", "tauri.conf.json");
/** tauri.conf.json `build.devUrl`, without a trailing slash. */
export const DEV_URL = JSON.parse(readFileSync(conf, "utf8")).build.devUrl.replace(/\/$/, "");

/**
 * "app" for the bundled frontend (tauri.localhost), "dev" for devUrl (the exe was built without
 * the `tauri/custom-protocol` feature, so it loads the Vite dev server), "other" for any other
 * loaded page (e.g. an error page), null while nothing is loaded yet.
 */
export function pageKind(url) {
  if (!url || url === "about:blank") return null;
  if (/^https?:\/\/tauri\.localhost\//.test(url)) return "app";
  if (url === DEV_URL || url.startsWith(`${DEV_URL}/`)) return "dev";
  return "other";
}
