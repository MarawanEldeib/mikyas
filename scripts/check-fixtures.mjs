// Rejects test fixtures that look like real user data (the Sept 2026 token leak
// came from committing captured machine data). Fixtures must be synthetic:
//  - UUIDs must start with 8 identical hex chars (e.g. 00000000-..., aaaaaaaa-...)
//  - no Anthropic token prefixes
//  - no real Windows user profile paths or the local username
import { readFileSync, existsSync } from "node:fs";
import { userInfo } from "node:os";

const files = process.argv.slice(2).filter((f) => existsSync(f));
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
