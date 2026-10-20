---
name: bento-release
description: >-
  Develop and release Bento (this repo): the dev/release branch model, the version rule
  (0.1.9 -> 0.2.0), cutting a GitHub release, checking the build, and the in-app updater.
  Use when the user asks to 发版 / 发布 / 出新版本 / release / publish / bump the version,
  asks which version or branch to use, or a release build fails.
---

# Bento release workflow

Bento is a Tauri 2 macOS app. Source: https://github.com/kohlarnhin/Bento (public).
Users install it from GitHub Releases and update it from inside the app. **Nothing is packaged
locally for distribution**: GitHub Actions builds every release.

## Branches

| Branch | Purpose |
|---|---|
| `dev` | All development. Default branch. Commit here. |
| `release` | Publishing only. Every push to it builds and publishes a release. Never commit on it directly; it only fast-forwards to `dev`. |

## Version rule

`MAJOR.MINOR.PATCH`; MINOR and PATCH each run 0–9 and carry at 10, MAJOR has no upper limit:
`0.1.0 → 0.1.1 → … → 0.1.9 → 0.2.0`, `0.9.9 → 1.0.0`, `9.9.9 → 10.0.0`.

The version lives in four places that must match: `package.json`, `src-tauri/tauri.conf.json`,
`src-tauri/Cargo.toml` and the `apphub` entry of `src-tauri/Cargo.lock` (the crate keeps its old
name on purpose). Never edit them by hand:

```bash
node scripts/bump-version.mjs --print   # current and next version
node scripts/bump-version.mjs           # write the next version to all four places
node scripts/bump-version.mjs --check   # verify they agree (CI runs this)
```

## Cutting a release

Only when the user asks for a release: a push to `release` publishes publicly and cannot be
taken back once people download it.

1. **Preconditions** on `dev`: working tree clean, everything committed, and these pass:
   `pnpm build`, `cd src-tauri && cargo test`, `node --test 'scripts/*.test.mjs'`.
2. **Version.** If tag `v<current version>` already exists (`git ls-remote --tags origin`),
   bump: `node scripts/bump-version.mjs`, then commit on `dev` as
   `chore(release): v<new version>` (this subject is left out of the release notes).
   The very first release, v0.1.0, needs no bump.
3. **Release notes come from commit subjects** since the previous tag (merge commits and
   `chore(release)` commits excluded). They are shown to users in the update dialog, so the
   subjects should read well; if they don't, tell the user before releasing.
4. **Publish:**
   ```bash
   git push origin dev
   git checkout release && git merge --ff-only dev && git push origin release
   git checkout dev
   ```
5. **Watch the build** (about 7 minutes; arm64 and x64 build in parallel):
   ```bash
   gh run list -R kohlarnhin/Bento --workflow release.yml --limit 1
   gh run watch <run-id> -R kohlarnhin/Bento --exit-status
   ```
6. **Verify the release:**
   ```bash
   gh release list -R kohlarnhin/Bento --limit 1 --json tagName,isLatest,isDraft
   gh release view v<version> -R kohlarnhin/Bento --json assets --jq '[.assets[].name]'
   curl -sL https://github.com/kohlarnhin/Bento/releases/latest/download/latest.json
   ```
   Expect seven assets: `Bento_<v>_arm64.dmg`, `Bento_<v>_x64.dmg`, `Bento_<v>_arm64.app.tar.gz`
   (+ `.sig`), `Bento_<v>_x64.app.tar.gz` (+ `.sig`) and `latest.json`; the release is marked
   latest; `latest.json` has the new version and non-empty signatures for `darwin-aarch64` and
   `darwin-x86_64`.
7. **Tell the user** the release URL and which DMG fits their Mac (the user's Mac is Intel → `x64`).
   Installed copies will offer the update on their next check (at launch and every 6 hours),
   or right away via 菜单「检查更新…」.

## When the release build fails

- The workflow stops before publishing if the versions disagree or the tag already exists
  ("bump the version on dev first"): fix on `dev`, then fast-forward and push `release` again.
- A build failure leaves no release behind: fix on `dev`, commit, fast-forward `release`, push.
  To re-run without new commits: `gh workflow run release.yml -R kohlarnhin/Bento --ref release`.
- If a release `v<x>` was created but is incomplete (the publish job failed after
  `gh release create`), delete it before re-running:
  `gh release delete v<x> -R kohlarnhin/Bento --cleanup-tag --yes`. Ask the user first if the
  release is already marked latest, because installed copies may be updating from it.

## Signing key (never expose)

Updates are verified with a minisign key pair:
- private key `~/.tauri/bento.key`, its password `~/.tauri/bento.key.password`, public key
  `~/.tauri/bento.key.pub` (the same public key is embedded in `src-tauri/tauri.conf.json`);
- repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

Never print, commit, or pass the private key or password to another agent. If the key is lost,
installed copies can no longer verify updates: generate a new pair, update the secrets and the
pubkey, and the user must reinstall manually once.

## Local builds

`./start.sh build` makes a local test build without updater artifacts. It is for checking the
bundle only, never for distribution. Run any local build with a temporary `HOME` and never
against `~/.apphub/apphub.db` unless the user asks.

## Commits

Commits here must be authored as `kohlarnhin <45386057+kohlarnhin@users.noreply.github.com>`.
The user's global `~/.gitconfig` provides that identity by default, so don't add a repo-local
`user.email`. Check with
`git config user.email` before committing if in doubt.

## Installing (for the user)

The app is ad-hoc signed and not notarized. After copying `Bento.app` to 应用程序, the first
launch on macOS 15 needs 系统设置 → 隐私与安全性 → 仍要打开, or
`xattr -dr com.apple.quarantine /Applications/Bento.app`. Updates installed from inside the app
don't need this again.
