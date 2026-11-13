# ADR 0029 — Organization suggestions and clutter detection

**Status:** accepted (M7b)

## Context
The spec asks for suggestions for one folder: group items into subfolders, rename them, tag them, and clean up
clutter (old downloads, large unused files, screenshot piles, near-duplicates). Three rules constrain it:
- nothing is ever applied automatically;
- the user sees a before/after preview and accepts all, some, or none;
- every applied change can be undone.

A local model is the default. A bring-your-own-key cloud model is optional, with a clear warning and a
per-folder opt-in.

## Decision
- **Domain:**
  - `Plan` of `Suggestion`s: `MoveInto(folder)`, `Rename(name)`, `AddTags(tags)` or `Trash(reason)`, each with
    a reason line and its source (rules, local model, cloud model).
  - `resolve(plan, accepted, existing)` turns the accepted ones into concrete steps: folders to create, trash,
    moves, tags. The preview shows exactly what applying does.
  - Resolution rules: the Trash wins over other changes to the same item; a move and a rename combine; names
    that would clash are numbered (`report 2.pdf`); names freed by items moving away are reused.
- **Analysis** (`AnalyzeFolder`): reads only the items directly in the folder, and changes nothing.
  1. **Clutter** (rules, `ClutterPolicy`):
     - installers older than 14 days;
     - anything in Downloads untouched for 90 days;
     - files over 500 MB unchanged for a year;
     - exact and look-alike duplicates, using the M7a duplicate finder restricted to the folder's own files. The
       oldest copy stays, and a name without a copy marker (` (1)`, ` copy`, ` 2`) wins a tie.
  2. **The cloud model**, if the user opted this folder in and set a key: groups, renames, tags.
  3. **Local rules**, filling the gaps:
     - five or more screenshots → `Screenshots/`;
     - months with eight or more photos, when there are several months → `Photos 2026-03/`;
     - camera names → the moment the photo was taken (`2026-03-14 15.09.26.jpg`, from EXIF `DateTimeOriginal`,
       else the file's date);
     - generic document names (`scan0001`, `Untitled`, `document (3)`) → the document's first line;
     - keywords in five languages → tags (Invoice, Receipt, Contract, Tax, Payslip, CV, Ticket).
  4. **The local language model** (M6's e5, if installed), when the cloud model wasn't used:
     - documents whose first passages are alike go into one folder, by average-linkage clustering at the
       model's `grouping_threshold`;
     - the folder is named after the word most distinctive to the group;
     - only documents that actually contain that word join, which keeps short look-alike texts out.

     The threshold for e5 is 0.80, measured on three topics in four languages (`real_models.rs`): the topics
     separate exactly from 0.78 to 0.81.
- **Applying** (`ApplySuggestions`): one job and one Undo step (`OperationKind::Organize`).
  - **Order:** create folders → Trash → moves and renames (the existing executor, which handles swaps) → tags.
    The tags merge with an item's existing ones.
  - **Undo** runs the inverse: tags restored (`Operation::TagsChanged` → `Action::SetTags`), items moved back
    and restored from the Trash, then the now-empty folders trashed.
  - Targets inside existing subfolders that are taken by then get numbered names.
- **Tags are native and local only** (user decision): Finder tags on macOS and `user.xdg.tags` on Linux.
  - **macOS:** a binary property list of `Name` / `Name\n<colour>` in `com.apple.metadata:_kMDItemUserTags`;
    colours of tags that stay are kept.
  - **Linux:** comma-separated values in `user.xdg.tags` (Dolphin, Baloo).
  - **Where:** `TagEditor` is a provider role (`EditTags`), implemented by the local disk only. Tags are never
    suggested for servers, buckets or drives.
- **The cloud model** (`OrganizationModel`, `adapter-cloud-models`; user decision: Anthropic + OpenAI-compatible):
  - **Anthropic:** the Messages API over raw HTTP (no Rust SDK), `anthropic-version: 2026-10-21`.
    - The answer is constrained with structured outputs (`output_config.format`, JSON schema) at effort
      `medium`. Current Claude models refuse forced tool use, so no tool is forced.
    - `fallbacks: "default"` is sent for models that support it on the Claude API.
    - `stop_reason` `refusal` and `max_tokens` are reported as such.
  - **OpenAI-compatible:** `chat/completions` with `response_format` (JSON schema). A server that rejects
    `response_format` is asked again with the schema in the prompt only. A code fence around the JSON is
    tolerated.
  - **Errors:** 429, 5xx and 529 are retried twice, honouring `retry-after`. 401/403 means "the key was refused".
  - **The answer is never trusted:** unknown item ids, invalid names, invalid tags and names that clash with
    files are dropped. A rename always keeps the original extension.
  - **Failure:** if the cloud model fails, the user gets a note and the local suggestions.
- **What may be sent** (user decision): the folder's name, and for up to 200 items their names, kinds, sizes,
  dates, and the first 400 characters of documents' text.
  - Never whole files, never images, never paths, never anything outside the folder.
  - It's sent only for folders the user opted in, after seeing the recipient host and the exact text that would
    go out ("Show What Would Be Sent"). `PreviewCloudRequest` and the real request are built by the same
    function, which a test verifies.
  - The key lives in the OS keychain; the provider, model and opt-ins live in SQLite (migration 11).
    *Remove Cloud Model* deletes all three.
- **UI:**
  - **Organize window:** sections for clean-up, new folders, clearer names and tags, each with All/None, and
    per-item checkboxes. Nothing starts out ticked. Next to them is the before → after preview of the ticked
    ones, and *Apply N Changes*. The plan goes to `OperationsController.applyOrganization` as JSON, so the
    applied plan is the previewed one.
  - **Cloud Model dialog:** provider, model and key, per-folder opt-in, and the preview of what would be sent.

## Consequences
- **Short documents can still look alike to the model.** Requiring the folder's word keeps groups tight, at
  the cost of missing groups whose documents never name their topic; the cloud model covers those for
  folders the user opts in.
- **Large folders:** at most 200 documents are read and 500 photos' EXIF; analysis reads only the folder's own
  items, not its subfolders.
- **Cloud costs:** one request per analysis, paid to the user's provider. Nothing is cached.
