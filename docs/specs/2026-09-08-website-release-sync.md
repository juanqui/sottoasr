# Website release synchronization

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Approved

## Contents

1. Summary · 2. Problem · 3. Design · 4. Details · 5. Edge cases · 6. Files
7. Testing · 8. Migration · 9. Security · 10. Cost · 11. Tasks · 12. Status

## 1. Summary

Keep sottoasr.app aligned with public downloadable releases and restore its existing Cloudflare Git connection.

## 2. Problem Statement

The live site advertises 0.7.4; the branch contains 0.8.3 but no public 0.8.3 release exists yet. Cloudflare reports its GitHub connection disconnected despite automatic main deployments being enabled. Website cleanup copy still describes the retired LFM model.

## 3. Design Overview

Retain static Cloudflare Pages hosting. A small dependency-free browser module resolves GitHub's latest public release once per page load, then updates the badge and both download links together. Generic latest-release links remain usable without JavaScript or API availability.

## 4. Detailed Design

Keep the source version badge for release consistency checks inside a hidden wrapper. Reveal it only after the API returns a stable published semantic version with a nonempty, uploaded matching Apple Silicon DMG. Construct release URLs from the fixed repository and validated tag. Use a five-second timeout, no credentials, no referrer, no persistent cache or polling. Correct MiniCPM copy and deployment documentation. Add the website badge and focused website tests to existing configuration checks.

## 5. Edge Cases

Drafts, prereleases, malformed responses, missing DMGs, network errors and rate limits leave the generic link and hidden version unchanged. Publishing a new release updates subsequent page loads without rebuilding the website. Existing loaded pages update on refresh.

## 6. File Changes

`website/index.html`, `website/release.js`, `website/deployment.md`, `scripts/test-website.mjs`, and `scripts/ci-checks.sh`.

## 7. Testing Strategy

Exercise published-release rendering, both matching links, invalid/draft/prerelease/missing-asset responses and network failure using the real HTML and module. Check the real public API and rendered browser page. Validate existing configuration assertions and frontend build; application inference code is unchanged.

## 8. Migration Plan

Repair only the existing Sotto Git integration, retaining main as production and website as output. Verify a branch preview after pushing the change. Production follows the PR merge; the badge follows publication, which remains a separate release action.

## 9. Security Considerations

Only public release metadata is fetched. No app data, browser credentials or tokens are sent. API strings are assigned through textContent, never HTML. GitHub account verification must be completed by the user.

## 10. Cost Analysis

One bounded public API request per page load. No new backend, hosting service or dependency.

## 11. Implementation Tasks

- [x] Implement release-aware badge and correct model copy.
- [x] Add website checks and document actual hosting/release behavior.
- [x] Verify browser rendering, fallback cases and build; update PR.
- [ ] Restore existing Git connection and verify Cloudflare preview deployment.

## 12. Implementation Status

Research: Cloudflare dashboard confirms main production, automatic deployments enabled, output website, and disconnected Git account. GitHub latest-release API returns 0.7.4 with an uploaded matching DMG. Primary references: [GitHub release API](https://docs.github.com/en/rest/releases/releases#get-the-latest-release) and [Cloudflare Git integration](https://developers.cloudflare.com/pages/configuration/git-integration/).

Review 1 — Assumptions: confirmed the latest-release endpoint excludes drafts/prereleases and the current uploaded DMG uses the versioned naming convention. Keep explicit response validation so incomplete publication never reveals a misleading version.
Review 2 — Completeness: keep both fallback links functional, hide only the numeric badge, verify all response fields before mutating the DOM, and cover API failure as well as unpublished releases. Restoring Git access is limited to the existing Sotto repository; any broader permission request requires review.
Review 3 — Actionability: preserve the existing version-bump badge literal, use the current Node/jsdom tooling for focused DOM tests, and run those checks through both existing CI workflows without changing app builds or release publication policy. Approved for implementation.

Local verification: four real-HTML/jsdom tests pass, including nine invalid/incomplete release variants and three failure paths. Configuration assertions and Vite build pass. Chrome renders the live API result v0.7.4 and both matching release links; screenshot confirms download layout. GitHub sudo verification is pending for installation 118245048, so Git connection repair and Cloudflare preview remain unverified.

Connection follow-up: account-holder GitHub verification completed. GitHub installation 118245048 is active with its existing repository access. Cloudflare no longer reports a disconnected account, and main/website automatic production routing is unchanged. A fresh documentation commit will verify preview delivery without touching the GitHub installation or publishing the app.
