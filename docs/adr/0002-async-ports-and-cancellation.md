# ADR 0002 — Native async ports with object-safe twins; runtime-free cancellation

**Status:** accepted (M1)

## Context
Ports must be async (network providers), object-safe (a registry holds heterogeneous providers), and free of
tokio types (ADR 0001).

## Decision
- Each role port is a trait using native `impl Future`/`impl Stream` returns with explicit `Send + 'static` bounds
  (for example, `Lister::list`). Adapters write ordinary `async fn`s.
- Each role has a `Dyn*` twin (`DynLister`, `DynWatcher`) returning `BoxFuture`/`BoxStream`. A blanket
  `impl<T: Lister> DynLister for T` means adapters never implement the twin by hand.
- Listing streams are `'static`. A use case can hand them to any executor without borrowing the provider.
- Cancellation is a pure-std value type in `application` (`CancellationSource` owns it, `CancelToken`
  observes it). This replaces the planned `tokio_util` token behind a port: one implementation, no runtime
  dependency, and it works with `futures-executor` in tests. Every long operation takes a `CancelToken`, and the
  contract requires a cancelled stream to end with exactly one `Err(Cancelled)`.
- Time is a port (`Clock`). `adapter-tokio-runtime` implements it, and tests use `InstantClock`.

## Consequences
- Boxing costs one allocation per call/stream. That's negligible next to IO.
- Adding a role means a trait, a twin and a blanket impl: about 20 lines of mechanical code.
