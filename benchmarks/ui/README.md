# Synthetic UI benchmark

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

This standalone harness measures the frontend served by Vite with generated settings and transcript history. It opens an isolated headless Chrome, replaces Tauri IPC before app code runs, and blocks requests outside the selected loopback server. It cannot record audio, invoke Rust, inspect real history, or change installed preferences.

## Table of Contents

1. [Run](#run)
2. [Compare consistently](#compare-consistently)
3. [Native verification boundary](#native-verification-boundary)
4. [Production CSP smoke](#production-csp-smoke)

## Run

Use Node 20.19+ or 22.12+ (the project's Vite requirement) and a local Chrome installation. Playwright is optional benchmark tooling, not an app dependency. The original measurements used Playwright 1.62.1 and Chrome 152.0.7977.77. To reuse an existing Playwright installation, set `SOTTO_UI_PLAYWRIGHT` to its absolute `index.mjs` path. Otherwise install it outside the repository:

```bash
npm install --prefix /tmp/sotto-ui-tools --no-package-lock --ignore-scripts playwright@1.62.1
```

In one terminal, from the source checkout being measured:

```bash
npm run dev -- --host 127.0.0.1 --port 14517 --strictPort
```

In another, from this repository root:

```bash
set -o pipefail
SOTTO_UI_PLAYWRIGHT=/tmp/sotto-ui-tools/node_modules/playwright/index.mjs \
SOTTO_UI_LABEL=current \
node benchmarks/ui/run.mjs 2>&1 | tee /tmp/sotto-ui-current.txt
```

The script prints its temporary artifact directory. JSON includes browser/Node versions, platform, source fingerprint, fixture dimensions, timing samples, mounted nodes, draw/RAF counts, and IPC commands. Screenshots contain synthetic settings only. Unknown IPC or browser errors fail the run rather than producing a misleading result. No project dependency, lockfile, generated snapshot, or machine-specific path is checked in.

| Environment variable | Default / meaning |
| --- | --- |
| `SOTTO_UI_SERVER` | `http://127.0.0.1:14517`; must be loopback |
| `SOTTO_UI_SOURCE` | Current directory; point to the **actual served checkout** for its `src/` content fingerprint |
| `SOTTO_UI_LAYOUT` | `current`; use `legacy` for the original single-page Settings/all-history UI |
| `SOTTO_UI_LABEL` | `current`; output name, independent of layout |
| `SOTTO_UI_OUTPUT` | Fresh temporary directory; use a new directory for each comparison |
| `SOTTO_UI_PLAYWRIGHT` | Optional absolute path to an existing Playwright `index.mjs`; otherwise normal package resolution |
| `SOTTO_UI_BROWSER_EXECUTABLE` | Optional browser executable; otherwise Playwright's installed Chrome channel |

## Compare consistently

Keep the old source in a separate checkout or copied frontend and serve it on a second port. Run the same harness with `SOTTO_UI_LAYOUT=legacy`, that server URL, and its correct `SOTTO_UI_SOURCE`; do not reset a working tree or reuse a label to select behavior. Record the package-lock revision and machine used with shared results. The fingerprint covers `src/`, not installed dependencies or Vite configuration.

Run serially while the machine is otherwise quiet. Do not overlap with ASR/LLM inference, model downloads, Rust builds, or another browser benchmark. For a claimed latency improvement, keep the browser version, hardware, fixture, and serving mode fixed; repeat paired runs only when needed to distinguish variance from the suspected effect.

| Exercise | Interpretation |
| --- | --- |
| 500 / 5,000 history items and five search edits | Full search, DOM bounds, input dispatch to the second animation frame |
| 0 / 200 replacement entries and five edits | Editing responsiveness and initialization IPC; JSON names the actual edited control |
| One second of settled idle overlay | Continuing RAF requests and waveform bar draws; expected current result is 0 / 0 |
| Ten synchronous bursts of 300 audio-level events | Callback scaling stress test; not realistic microphone cadence or end-to-end latency |

The empty Settings control changed during redesign, so its before/after timing is **not** a same-control speed comparison. Page-ready timing includes browser/Vite cold-start variance. The second-frame timing is a responsiveness proxy, not physical input-to-photon latency. Chromium long tasks exclude native inference and do not establish WKWebView CPU or energy usage. The instrumented `roundRect` count targets the present waveform implementation; adapt it if drawing primitives change.

## Native verification boundary

Use Safari/WebKit Inspector's Timelines on an isolated development app for packaged rendering, script, CPU, and allocation evidence. Compare idle, recording, and a 5,000-entry synthetic history separately. Use Accessibility Inspector, VoiceOver, full keyboard access, larger text, and Reduce Motion for the actual WKWebView/NSPanel behavior. This harness does not certify those native paths. See [the research record](../../docs/research/2026-09-08-settings-performance.md) for measured results and primary-source guidance.

## Production CSP smoke

`production-smoke.mjs` starts its own loopback server for `dist/` and applies the CSP from `src-tauri/tauri.conf.json`. It checks all four Settings sections, default-off cleanup preparation/cancellation/save, model/settings read failures, native-link IPC routing, 5,000-entry history search, overlay snapshot recovery, Reduce Motion, and original recording start time. Each page must report zero CSP violations, unknown IPC commands, or browser errors.

```bash
set -o pipefail
npm run build 2>&1 | tee /tmp/sotto-ui-production-build.txt
SOTTO_UI_PLAYWRIGHT=/tmp/sotto-ui-tools/node_modules/playwright/index.mjs \
node benchmarks/ui/production-smoke.mjs 2>&1 | tee /tmp/sotto-ui-production-smoke.txt
```

The optional Playwright/browser configuration is the same as above. No Vite development server is needed. Output goes to a fresh temporary directory; screenshots show synthetic content. This uses raw production assets and the configured CSP; Tauri's resource/nonce rewriting and native WKWebView are separate integration layers. Passing this test does not claim that synthetic IPC invoked real model setup, native focus, or disk persistence.
