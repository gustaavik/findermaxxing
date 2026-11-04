# ADR 0015 — Cross-provider transfers stream; no Trash means a different delete

**Status:** accepted (M4)

## Decision
- **Routing:** a transfer resolves a `Route { source, dest, same_mount }`. Within one mount it uses `Copier`/`Mover`;
  if they return `Unsupported` (or the mounts differ) it streams `Reader` → `Writer` in chunks with progress and
  cancellation. Nothing is buffered whole, so a 50 GB object uses constant memory.
- **Writers commit atomically:** local writes go to a hidden partial file and are renamed into place without
  replacing; S3 writes become visible only on PUT/Complete. A cancelled or failed transfer leaves no partial item.
- **Moves across providers** copy, and delete the source only after the copy has committed. A failure before the delete
  leaves the source intact.
- **`NoTrash` policy** in the executor:
  - user-initiated "Move to Trash" on a provider without `Trash` is refused, and the UI offers permanent delete
    with a confirmation instead;
  - undoing a copy onto such a provider (`DeleteCreated`) deletes what the copy created.
- **`Operation::Deleted` is irreversible.** It's recorded so the history is honest, but Undo isn't offered and
  undoing a step whose result is irreversible doesn't push a Redo.
- **`Conflict::replace_is_permanent`** tells the prompt that Replace on that destination can't be undone.

## Consequences
- The contract suite gained `stream_contract_tests!` (7 cases, including a 17 MiB round trip that crosses the
  multipart threshold) and `renamed_folders_leave_nothing_behind`.
- Undo of an S3 copy is a real delete; undo of a cross-provider move is another cross-provider move back.
