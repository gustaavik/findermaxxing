# ADR 0003 — Role-segregated ports, capability sets, scheme registry

**Status:** accepted (M1)

## Context
Providers differ: S3 has no atomic rename or real directories, archives are read-only, and some providers can't
watch. Callers must not special-case providers (Liskov), and adding one must not edit existing code (Open/Closed).

## Decision
- Small role traits, each added with the milestone that uses it:
  - M1: `Lister`, `Watcher`;
  - M2: `Copier`, `Mover`, `Remover`, `FolderMaker`, `Trasher`, `Launcher` (ADR 0008);
  - M4: streaming `Reader`/`Writer` for cross-provider copies (ADR 0015), and `MetadataEditor`, `Presigner`,
    `VersionLister` for remote items (ADR 0014);
  - M5a: resumable writes on the stream roles (ADR 0019); caching and offline access are decorators over any
    provider, not roles (ADR 0020).
- `MountHandle` holds an `Option<Arc<dyn Role>>` per role. A missing role yields
  `VfsError::Unsupported(capability)`, so a read-only provider implements nothing it can't do.
- `Capabilities` is a value set: role presence (List, Watch, …) is derived automatically, and semantic
  guarantees (`AtomicRename`, `RealDirectories`, …) are declared by the provider. Callers branch on these values.
- `ProviderRegistry` maps a URI `Scheme` to a `ProviderFactory`. A new provider is a new adapter crate plus one
  registration line in `app`.
- A shared **contract suite** (`vfs-contract-tests`) defines the observable behaviour: error variants,
  batch limits, cancellation, stat/list agreement and watcher latency. Every adapter, and the in-memory fake
  used by use-case tests, must pass it.
- Renames are reported as `Removed` + `Upserted`. Lost events produce `Rescan`.

## Consequences
- The fake is held to the same contract as real adapters, so use-case tests stay trustworthy.
- `ChangeEvent` deviates from the plan's Created/Modified/Renamed set: two operations are enough for every consumer and coalesce cleanly.
