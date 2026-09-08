# SottoASR website deployment

- **Version:** 1.1
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. Hosting · 2. Release synchronization · 3. Verification

## 1. Hosting

The existing Cloudflare Pages project is **sottoasr**, connected to
**juanqui/sottoasr**, serving **https://sottoasr.app** and
**https://sottoasr.pages.dev**. Production branch: `main`. Build command: empty.
Build output: `website`. This plain HTML/CSS/JavaScript site is separate from the
Tauri frontend; `npm run build` does not deploy it.

The dashboard confirms automatic production deployments are enabled. On
September 8 it also reported that the Git account was disconnected. Restoring
that connection requires GitHub account verification and access to this existing
repository. Do not assume a pushed change deployed: check Cloudflare's deployment
status and the served page. Keep the current domain and main-branch routing.

## 2. Release synchronization

Cloudflare deploys site content when changes merge to main. Application builds
create a **draft** GitHub release; publication remains a separate action.

`release.js` reads GitHub's latest public release once per page load. It reveals
the version and updates both download links only for a stable published release
with a matching uploaded Apple Silicon DMG. Publishing a new release therefore
updates the displayed version on subsequent page loads without another website
deployment. The request has a five-second timeout, sends no credentials or
referrer, and stores no persistent browser cache.

Until that check succeeds, visitors see generic latest-release links and no
unverified version number. Disabled JavaScript, rate limits and network failures
leave downloading usable. The hidden source badge still tracks the app manifest
and is checked by CI, preserving the normal release version-bump process.

## 3. Verification

Run `bash scripts/ci-checks.sh` for version consistency and website behavior tests.
After pushing, inspect the Cloudflare preview check for the exact commit. After
merging and publishing, inspect `sottoasr.app/#download` and follow its release
link to confirm the matching DMG is available. No production deployment or public
app release is implied by a successful local build.

References: [Cloudflare Git integration](https://developers.cloudflare.com/pages/configuration/git-integration/)
and [GitHub latest-release API](https://docs.github.com/en/rest/releases/releases#get-the-latest-release).
