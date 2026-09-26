// Pre-commit secret scan of the staged changes (lefthook). If gitleaks is not installed, say so
// loudly and let the commit through: CI runs gitleaks over the whole history on every push, so a
// missing local binary must not block work, but it should never go unnoticed either.
import { spawnSync } from "node:child_process";

const args = ["git", "--pre-commit", "--staged", "--redact", "--no-banner", "--config", ".gitleaks.toml"];
const run = spawnSync("gitleaks", args, { stdio: "inherit" });

if (run.error && run.error.code === "ENOENT") {
  console.warn(
    "\n  WARNING: gitleaks is not installed, so the staged changes were NOT scanned for secrets.\n" +
      "  Install it (winget install gitleaks) - CI will still scan this commit when it is pushed.\n",
  );
  process.exit(0);
}
if (run.error) {
  console.error(`gitleaks could not run: ${run.error.message}`);
  process.exit(1);
}
process.exit(run.status ?? 1);
