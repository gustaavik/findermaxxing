# ADR 0010 — Undo/redo as an operation journal with pure inverses

**Status:** accepted (M2)

## Decision
- Every mutating use case records what *actually happened* as a domain `Operation` (`Copied`, `Moved`, `Trashed`, `Restored`, `CreatedFolder`, `Sequence`). A failed or stopped job still records its completed part, so a half-finished copy can be undone.
- `Operation::inverse()` is a pure domain function returning an `Action` (`Trash`, `Restore`, `Move`). Undo runs the inverse; **what the undo did becomes the redo entry**, and redo is that entry's inverse. One mechanism serves both directions.
- History entries carry the originating `OperationKind`, so the menu says "Redo Copy" even though the redo entry is technically "restore the trashed copies".
- **Undo never destroys data:**
  - undoing a copy *trashes* the copies;
  - replacing an item *trashes* the old one, so undo restores it;
  - undo never prompts, and any name clash fails instead of overwriting;
  - if an undo changes nothing (the item is gone), its entry is kept for a retry.
- Permanent delete is never journaled; the UI says so in its confirmation.
- One `OperationHistory` per window, capped at 100 entries.

## Proven by
`application/tests/file_operations.rs`: copy, undo, redo; replace, undo restores the original; partial failure is undoable; a blocked undo keeps its entry; rename and new folder undo. `domain::operation` covers the inverse algebra.
