# ADR 0013 — Bulk rename: pure planner, staged execution, one undo step

**Status:** accepted (M3)

## Decision
- **Rules live in `domain::BulkRename`**:
  - Replace (plain text or regex with `$1` groups; plain-text replacements never expand `$`);
  - Add (before or after the name);
  - Format (`base 001`, start number and padding);
  - Change Case (lower, upper, title).
  Each rule edits the name without its extension by default; "Include extensions" switches that off for Replace and Change Case. `regex` is a domain dependency: it's pure Rust, does no IO, and patterns are a business rule.
- **`BulkRename::plan` is pure** and powers the live preview. It flags names that aren't text, invalid names, duplicates within the batch, and names taken by items that *aren't* moving away. That last check repeats until stable, so shift chains (`f 2→f 3`, `f 3→f 4`) are allowed while true clashes are not. The preview is exactly what will happen; problem items are skipped.
- **Execution (`BulkRenameEntries`)** uses `operations::relocate::rename_batch`: direct renames when no target is another item's source, otherwise **two phases through unique staging names** (`.findermaxxing-renaming-N-M`), with best-effort rollback on failure. Once staging starts, the batch isn't cancellable, so items are never left under temporary names. Undo of a swap goes through the same staging, because the executor detects permutations.
- One `Moved` journal entry of kind `Rename` means one undo restores every name.
