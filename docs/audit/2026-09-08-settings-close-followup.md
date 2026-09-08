# Settings Close Button Follow-up

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## 1. Cause and fix

The installed Tauri JavaScript implementation of `Window.onCloseRequested` calls
`Window.destroy()` after an accepted close request. Settings registered this
listener to protect unsaved changes, but its capability allowed `close`, not
`destroy`; `core:window:default` does not grant destruction. Thus the native X and
confirmed Save/Discard paths reached a denied command. The earlier component
tests replaced the Window API and missed this implicit SDK call.

`src-tauri/capabilities/settings-close.json` grants `core:window:allow-destroy`
only to the Settings window. The existing draft-confirmation handler and native
menu-bar lifecycle remain unchanged. A new SDK-level regression exercises the
actual installed close-request wrapper with configured capability data. Component
regressions cover unchanged X, Keep editing, Discard, Save and close, and failed
persistence keeping the draft open.

Native inspection also found a stale setup reminder after cleanup enable had
already saved. Settings now acknowledges that successful enabled preference save
and clears only its "Ready. Save" reminder. Failed saves and unrelated saves
while preparation is pending retain the correct setup state.

## 2. Task and verification record

- [x] Trace the native window listener, installed SDK close implementation, and generated permission defaults.
- [x] Add the narrowly scoped capability and close lifecycle regression tests.
- [x] Clear the setup reminder only after a successful enabled preference save, with success/failure/pending regressions.
- [x] Run frontend tests and Svelte/type checks with captured output: all 145 tests across 17 files pass; 0 Svelte errors or warnings.
- [ ] Post-install native X retest: blocked by computer-use access to the menu-bar-only app; see below.

This follow-up does not modify the implemented feature specifications. It does not
build or replace the installed application; native confirmation belongs to the
parent agent's integration step.

Root installed signed 0.8.1 and verified startup, settings/history preservation,
and the bundled capability/source match. Native X was reproduced before updating;
after relaunch the tool timed out before it could reopen Settings from the tray.
The final SDK regression passes but does not substitute for that unperformed mouse
test. See the [installation record](../journals/2026-09-08-local-081-install.md).

Verification logs: `/tmp/sotto-followup-frontend-tests-verified.txt` and
`/tmp/sotto-followup-frontend-check-sdk-final.txt`. The SDK regression fixture was
corrected to use Vite capability imports and the real Tauri internal IPC boundary;
earlier fixture-only failures are retained in the preceding follow-up logs. No
production workaround or broader permission grant was introduced to satisfy tests.
