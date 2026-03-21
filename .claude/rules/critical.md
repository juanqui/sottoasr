# Critical Rules

These rules are non-negotiable. Violating any of them can cause data loss, security incidents, or broken deployments.

## Security

- NEVER commit secrets — API keys, tokens, credentials, `.env` files, or any sensitive configuration. If you encounter a secret in source code, flag it immediately and help the user remove it from history.
- NEVER modify model configuration, API keys, or download locations without explicit user permission.

## Data Safety

- NEVER delete or reset database data without explicit user consent. This includes migrations that drop tables or columns.
- NEVER run destructive operations (file deletion, git reset --hard, database wipes) without confirming intent with the user.

## Git Discipline

- NEVER commit or push to git unless explicitly asked. Stage and show changes, then wait for the user to confirm.
- ALWAYS run the build before pushing to catch compilation errors. A push that breaks CI is a preventable mistake.
- Use `tee` to capture build/test output so it can be reviewed without re-running expensive commands.

## Verification

- Work is not done until it is verified: build passes, linter is clean, tests pass.
- NEVER assume you know how something works — always verify by reading the actual code, config, or documentation.
- All significant work products (specs, designs, implementations) must be reviewed iteratively. Specs require a minimum of 3 review passes before implementation begins.

## Debugging

- No rash decisions during debugging. Follow the scientific method: formulate a hypothesis, validate it with evidence, then apply the fix.
- NEVER shotgun-debug by making multiple speculative changes at once. Change one thing, verify, then proceed.

## Efficiency

- NEVER run builds, tests, or linters multiple times when once suffices. Capture output with `tee` (e.g., `cargo build 2>&1 | tee /tmp/build-output.txt`) and refer back to it.
- NEVER re-read files you have already read in the current session unless the file has been modified.

## Planning

- Write a spec first when the feature is complex. If you are unsure whether a feature is complex, it probably is.
- Track all implementation work with tasks. Every unit of work should be accounted for.
