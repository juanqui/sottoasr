# Spec-Driven Development Workflow

Every non-trivial feature follows this lifecycle. Do not skip phases — each one exists to prevent costly mistakes downstream.

## Phase 1: Research

Before writing a single line of spec, understand the problem space.

1. Read the requirements carefully. Clarify ambiguities with the user before proceeding.
2. Explore the existing codebase to understand current architecture, patterns, and constraints.
3. Map the blast radius — which files, modules, and systems will be affected?
4. Research external dependencies, libraries, or APIs involved. Use MCP tools (web search, documentation lookup) to get current information rather than relying on training data.
5. Document findings as you go. Raw notes are fine at this stage.

**Output:** Enough understanding to write a grounded spec. If the research reveals the feature is more complex than expected, flag this to the user.

## Phase 2: Specification

Create the spec document at `docs/specs/YYYY-MM-DD-slug.md` following the structure defined in the docs rule.

Key principles:
- Ground every claim with evidence. If you say "library X supports Y," cite where you verified this.
- Be specific about file changes — list exact paths and describe what changes in each.
- Write implementation tasks as an ordered, dependency-aware checklist.
- Set the status to "Draft."

## Phase 3: Review

Perform a minimum of 3 sequential review passes. Each pass has a distinct focus:

### Pass 1: Assumption Validation
- Are all technical claims accurate and verified?
- Are there hidden assumptions about the environment, OS, or runtime?
- Do the referenced APIs, libraries, and tools actually work as described?
- Are version constraints specified where they matter?

### Pass 2: Completeness
- Are all edge cases identified and addressed?
- Is the error handling strategy complete?
- Are security implications covered?
- Is the migration plan sufficient (if applicable)?
- Are there missing tasks in the implementation checklist?

### Pass 3: Clarity and Actionability
- Could another developer implement this spec without asking questions?
- Are the tasks ordered correctly by dependency?
- Are diagrams clear and accurate?
- Is the testing strategy specific enough to execute?

After all passes, update the status to "In Review" or "Approved" as appropriate. Document any changes made during review within the spec itself.

## Phase 4: Experimentation (Optional)

When the spec involves unfamiliar technology or risky trade-offs:

1. Create small, self-contained experiments in `/tmp/experiments/`.
2. Each experiment should test one specific question (e.g., "Can cpal capture system audio on macOS 14?").
3. Compare approaches with measurable criteria (latency, memory, correctness).
4. Feed results back into the spec. Update the design if experiments reveal a better approach.
5. Clean up experiments after incorporating findings.

## Phase 5: Implementation

1. Break the spec into atomic, committable tasks.
2. Implement in the order defined by the spec's task list.
3. Commit incrementally — each commit should represent a coherent, buildable unit of work.
4. Follow the file changes table from the spec. If you need to deviate, update the spec first.
5. Write tests alongside implementation, not after.

## Phase 6: Verification

Before declaring the work complete, verify all of the following:

```bash
# Build
cargo build 2>&1 | tee /tmp/verify-build.txt

# Lint
cargo clippy -- -D warnings 2>&1 | tee /tmp/verify-clippy.txt

# Type check (frontend)
npm run check 2>&1 | tee /tmp/verify-check.txt

# Unit tests
cargo test 2>&1 | tee /tmp/verify-test.txt

# E2E tests (if applicable)
# npm run test:e2e 2>&1 | tee /tmp/verify-e2e.txt
```

Additionally:
- Perform manual verification of the feature's happy path.
- Verify edge cases identified in the spec.
- Confirm the implementation aligns with the spec. If it deviates, document why.

## Phase 7: Spec Maintenance

After implementation is complete:

1. Update the spec status to "Implemented."
2. Document any deviations from the original design in a "Deviations" section.
3. Add implementation notes that would help future developers understand decisions made during implementation.
4. If the feature will evolve further, create a design doc in `docs/designs/` as the new living document.
