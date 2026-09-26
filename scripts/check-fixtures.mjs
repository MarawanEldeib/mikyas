// Rejects test fixtures that look like real user data (the Sept 2026 token leak
// came from committing captured machine data). Fixtures must be synthetic:
//  - UUIDs must start with 8 identical hex chars (e.g. 00000000-..., aaaaaaaa-...)
//  - no Anthropic token prefixes
//  - no real Windows user profile paths or the local username
//
// With --data-names it only checks file NAMES: any widget data file (settings backups,
// wrap/state/alerts/... json, a backups/ dir) is rejected wherever it is, since one staged from a
// SOVA_DATA_DIR inside the repo would publish real usage data. .gitignore covers them too; this
// catches a forced add.
import { readFileSync, existsSync } from "node:fs";
import { basename } from "node:path";
import { userInfo } from "node:os";

const args = process.argv.slice(2);
const namesOnly = args[0] === "--data-names";
const files = (namesOnly ? args.slice(1) : args).filter((f) => existsSync(f));
const DATA_NAME = /^(settings-.*|wrap|state|alerts|positions|watchdog|update-check)\.json$/i;
const DATA_DIR = /(^|[\\/])(backups|\.sova-data)[\\/]/i;

if (namesOnly) {
  const bad = files.filter((f) => DATA_NAME.test(basename(f)) || DATA_DIR.test(f));
  if (bad.length) {
    console.error("Widget data files must never be committed:\n  " + bad.join("\n  "));
    process.exit(1);
  }
  process.exit(0);
}
const username = userInfo().username.toLowerCase();
const uuidRe = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;
const problems = [];

for (const file of files) {
  const text = readFileSync(file, "utf8");
  for (const m of text.matchAll(uuidRe)) {
    const head = m[0].slice(0, 8).toLowerCase();
    if (!/^(.)\1{7}$/.test(head)) problems.push(`${file}: non-synthetic UUID ${m[0].slice(0, 8)}…`);
  }
  if (/sk-ant-/i.test(text)) problems.push(`${file}: contains an Anthropic token prefix`);
  const lower = text.toLowerCase();
  if (username && username !== "tester" && lower.includes(username)) {
    problems.push(`${file}: contains the local username`);
  }
  if (/[a-z]:[\\/]{1,2}users[\\/]{1,2}(?!tester\b)[^\\/"]+/i.test(text)) {
    problems.push(`${file}: contains a real user profile path (use C:\\Users\\tester)`);
  }
}

if (problems.length) {
  console.error("Fixture check failed:\n  " + problems.join("\n  "));
  process.exit(1);
}
