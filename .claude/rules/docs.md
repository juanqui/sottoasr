# Documentation Standards

## Directory Structure

```
docs/
├── specs/        # Feature specifications (immutable once implemented)
│   └── YYYY-MM-DD-slug.md
├── research/     # Research notes and findings
│   └── YYYY-MM-DD-slug.md
├── journals/     # Iterative experiment logs (tuning, debugging, optimization)
│   └── YYYY-MM-DD-slug.md
├── audit/        # Code and architecture audits
│   └── YYYY-MM-DD-slug.md
└── designs/      # Design documents (living docs, updated over time)
    └── slug.md
```

- **Specs** are date-prefixed and represent a point-in-time decision. Once a spec is implemented, create a new spec for further changes rather than rewriting the original.
- **Research** documents capture investigation results, benchmarks, and external findings. Date-prefixed because they reflect knowledge at a point in time.
- **Journals** are date-prefixed logs of iterative work — prompt tuning cycles, parameter sweeps, debugging sessions. Each entry records what was tried, what was measured, and what was learned. Journals live in `docs/journals/`, not alongside benchmark code.
- **Audits** are date-prefixed reviews of code quality, architecture, security, or performance. They capture the state of the system at a point in time and list findings, recommendations, and remediation status.
- **Designs** are living documents (no date prefix) that evolve with the system. Examples: architecture overview, data flow diagrams, API surface.

## Document Format

Every document must start with:

```markdown
# Title

- **Version:** 1.0
- **Date:** YYYY-MM-DD
- **Status:** Draft | In Review | Approved | Implemented | Superseded
```

### Formatting Rules

- Include a Table of Contents if the document has 3 or more sections.
- Use numbered sections for specs and designs.
- Use tables for comparisons (e.g., evaluating libraries, trade-off analysis).
- Use Mermaid diagrams for flows, state machines, and architecture.
- Keep paragraphs short (3-5 sentences max).

## Spec Structure

A complete spec follows this outline:

1. **Summary** — One paragraph explaining what this spec covers and why.
2. **Problem Statement** — What problem does this solve? Who is affected?
3. **Design Overview** — High-level approach with a diagram if helpful.
4. **Detailed Design** — Implementation details, data structures, APIs.
5. **Edge Cases** — What can go wrong? How is each case handled?
6. **File Changes** — Table of files to be created, modified, or deleted.
7. **Testing Strategy** — Unit tests, integration tests, manual verification steps.
8. **Migration Plan** — If applicable: how to migrate existing data or users.
9. **Security Considerations** — Threat model, permissions, data handling.
10. **Cost Analysis** — Performance impact, resource usage, dependencies added.
11. **Implementation Tasks** — Ordered checklist of work items.
12. **Implementation Status** — Updated during and after implementation.

## Critical Rules

- **One file per spec.** Never split a spec into separate review, summary, or notes files. Everything lives in a single document.
- **Single source of truth.** Each feature or decision has exactly one authoritative document. Do not duplicate information across specs.
- **No orphaned docs.** If a spec is superseded, update its status to "Superseded" and link to the replacement.
