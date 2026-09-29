# Verifying a Mikyas release

Official Mikyas builds come only from this repository's
[Releases page](https://github.com/MarawanEldeib/mikyas/releases). This page explains how they are
built, how you can check that a download is one of them, and what these checks cannot do.

## What is verified

Each release has two files:

- `Mikyas_<version>_x64-setup.exe`: the installer. It contains the widget (`mikyas.exe`), the
  statusline helper (`mikyas-capture.exe`), the terms, the privacy notes and the third-party
  notices.
- `SHA256SUMS`: the SHA-256 hash of the installer.

Both files carry a signed **build provenance attestation**. It is a record, signed through
GitHub and Sigstore, that states which repository, commit, tag and workflow run produced the files.

## How official builds are made

1. The version is set in `package.json`, `Cargo.toml` and `src-tauri/tauri.conf.json`, and the
   release commit is pushed to `main`.
2. The owner pushes a `v<version>` tag. A repository ruleset lets only the owner create, move or
   delete release tags.
3. The `Release` workflow (`.github/workflows/release.yml`) runs on GitHub's own machines, from
   that tag. It:
   - refuses to run unless the tag is on `main` and matches the app version;
   - runs every test and security check from a clean state;
   - builds with the exact dependency versions in `Cargo.lock` and `package-lock.json`
     (`npm ci`, `cargo … --locked`), using actions pinned to exact commits;
   - writes `SHA256SUMS`;
   - waits for the owner's approval (the `release` environment);
   - attests both files and creates a draft release.
4. The owner adds the release notes and publishes the draft. Releases are immutable, so a
   published release's files and tag cannot be swapped afterwards.

## Check a download

**Hash** (PowerShell, no extra tools):

```powershell
Get-FileHash "$env:USERPROFILE\Downloads\Mikyas_*_x64-setup.exe" -Algorithm SHA256
```

The hash must match the line in `SHA256SUMS`. GitHub also shows each file's SHA-256 on the release
page.

**Provenance** (needs the [GitHub CLI](https://cli.github.com)):

```powershell
gh attestation verify "$env:USERPROFILE\Downloads\Mikyas_0.3.0_x64-setup.exe" --repo MarawanEldeib/mikyas
```

Change the file name to the version you downloaded. The check passes only for a file built by this
repository's workflow. It fails for a copy that was rebuilt, patched or repackaged elsewhere.

v0.1.0 and v0.2.0 were built by hand before this workflow was used. They have `SHA256SUMS`, but no
attestation.

## Code signing

The installer is not code-signed. A Windows code-signing certificate costs money every year, and
Mikyas is free. Windows SmartScreen may therefore warn on first run (**More info → Run anyway**).
The hash and the attestation above are how you confirm that a download is genuine.

## Building it yourself

Anyone can build from source for development (see "For developers" in the
[README](../README.md#for-developers)). Such builds work normally. They are simply not official:
they have no attestation, and their hash will not match the release. The app has no switch that
tells official and development builds apart, and it needs none.

## Limits

- The checks happen **before you install**, with the files and tools above. The app does not check
  itself at runtime. Anyone who modifies a program can also remove a check inside it, so a
  self-check would give no real protection. It could also lock real users out after an antivirus
  quarantine or an interrupted update.
- No client-side code can be made impossible to reverse engineer, and Mikyas's source is public
  anyway, so it is not obfuscated.
- A verified installer proves where the file came from. It does not prove that the code is free of
  bugs. What the app reads and writes is documented in [PRIVACY.md](../PRIVACY.md).
- Download Mikyas only from the Releases page. Copies elsewhere are not official, whatever they
  claim.
