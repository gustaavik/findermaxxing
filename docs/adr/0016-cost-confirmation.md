# ADR 0016 — Confirm transfers that may cost money

**Status:** accepted (M4)

## Decision
- **`domain::CostPolicy::assess(&TransferEstimate)`** is a pure rule. It warns only when a metered provider is
  involved and the transfer is **≥ 1 GB** or crosses **regions** (both regions known and different).
- The transfer scans first, then asks the **`CostConfirmer`** port. Answers: `Proceed`, `ProceedAndStopAsking`
  (sets `skip_cost_warnings` on every account involved, persisted) and `Cancel` (the job ends as cancelled,
  nothing was transferred).
- `QtCostConfirmer` checks the accounts' `skip_cost_warnings` before showing `CostDialog.qml`.
- **Undo and redo are unattended** (`JobContext::unattended`, `AcceptCosts`): the user already agreed once.

## Consequences
- The threshold is a value on the policy; a settings UI (M8) can change it without touching use cases.
- The estimate counts bytes, not requests, so a million tiny objects never warn. Revisit if request costs bite.
