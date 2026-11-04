# ADR 0019 — Transfers survive a quit or crash

**Status:** accepted (M5a)

## Context
A 40 GB upload cut short by a quit, crash or reboot should continue, not start over, and must never leave
billed leftovers (S3 multipart parts) or half-written files behind.

## Decision
- **`TransferJournal` port** (SQLite `transfer_journal`): `CopyEntries`/`MoveEntries` record each job before it
  starts and remove it when it ends: finished, failed or cancelled. A job that is still recorded at start-up was
  interrupted. Journal failures never fail the transfer.
- **Resumable writes** extend the stream roles:
  - `WriteSink::checkpoint()` hands out an opaque `ResumeState`. From then on, dropping the sink uncommitted
    keeps the partial data; `abort` still deletes it.
  - `Writer::resume(target, state)` returns a sink plus the offset it continues from; `Writer::discard` deletes
    the leftovers.
  - `Reader::read_from(offset)`: ranged GET (S3, WebDAV), seek (local, SFTP), positioned reads (SMB); the
    default reads and skips.
  - States per provider:

    | Provider | State | Offset on resume |
    |---|---|---|
    | S3 | multipart upload id | leading whole parts (ListParts) |
    | Local, SFTP, SMB | name of the hidden partial file | the partial file's size |
    | WebDAV | none (not resumable) | — |
- **Only files ≥ 16 MiB are journaled** (`RESUMABLE_SIZE`); smaller ones just start over.
- **`ResumeTransfer`** runs the job again in "earlier run" mode:
  - folders that exist are merged into; files of the source's size count as done (writes are atomic, so a file
    of the right size is complete);
  - sources already moved away are skipped;
  - the file that was in flight continues from its offset, unless its source changed (size or date), in which
    case the leftovers are discarded and it restarts;
  - cost warnings aren't asked again.
- **`DiscardTransfer`** deletes the in-flight leftovers and forgets the job.
- At start-up the window asks once: *Resume* or *Discard*.

## Consequences
- Proven by `resume_contract_tests!` (checkpoint, drop, resume, byte-identical result; discard leaves nothing)
  for the fake, local disk, S3, SFTP and SMB, and by `application/tests/resume_servers_offline.rs`.
- A job is resumed as a whole; there is no per-file "resume later" while the app keeps running.
- Leftovers of a job that is never resumed or discarded stay until the user decides (the prompt returns at each start).
