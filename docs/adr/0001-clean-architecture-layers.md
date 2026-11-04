# ADR 0001 — Clean Architecture layers, enforced by a test

**Status:** accepted (M1)

## Context
The product has to support many storage providers, AI features and two desktop platforms over years.
Infrastructure churn (Qt versions, SDKs, cloud APIs) must not ripple into business rules.

## Decision
Five layers with dependencies pointing inward only:
`domain` ← `application` ← adapters / `presentation` ← `app` (composition root).

- `domain` depends only on `thiserror`.
- `application` depends on `domain`, `thiserror` and the runtime-agnostic `futures` traits.
- Adapters and `presentation` depend on `application` + `domain`, never on each other.
- Only `app` names concrete adapter types.

`crates/arch-tests` turns this into a failing test. It reads `cargo metadata`, checks the workspace layer
table, and rejects any *transitive* normal dependency from `domain`/`application` on `cxx*`, `qt-*`, `tokio*`,
`rusqlite`, `aws-*`, `opendal`, `notify`, `ort`, `reqwest` or `hyper`. Unit tests inside the crate prove
that the checker catches violations.

## Consequences
- A shortcut such as `presentation → adapter-local-fs` breaks CI instead of slipping through review.
- Dev-dependencies are exempt, so use-case tests can use `futures-executor` and adapter tests can use tokio.
- cargo-deny guards licenses/sources/advisories. It does not express "crate X may not depend on Y" as precisely, so it doesn't replace this test.
