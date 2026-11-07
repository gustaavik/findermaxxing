# ADR 0026 — Local embedding models on candle

**Status:** accepted (M6)

## Context
Semantic search (ADR 0025) needs a multilingual text model and a model that puts images and text queries in one
space. It must run on CPU on macOS and Linux, ship in a closed-source app, and never send files or queries
anywhere.

## Decision
- **Runtime:** candle 0.11 (pure Rust: `candle-core`, `candle-nn`, `candle-transformers`) and `tokenizers`
  without default features. This means no ONNX Runtime or libtorch to bundle, sign or link.
- **Models**, all MIT or Apache-2.0, pinned to exact Hugging Face revisions, each file checked by SHA-256
  (`manifest.rs`):

  | Pack | Models | Download | On disk |
  |---|---|---|---|
  | Text (asked for when AI search is turned on) | `intfloat/multilingual-e5-small`, 384 dims, ~100 languages | 487.7 MB | 241 MB |
  | Images (a second, separate opt-in) | `openai/clip-vit-base-patch32` (vision half) and `sentence-transformers/clip-ViT-B-32-multilingual-v1` (text side, 768 → 512 dense layer) | 1.15 GB | 428 MB |

- **Installing:**
  - Nothing is downloaded until the user agrees in a dialog that shows the size and the source.
  - `HuggingFaceInstaller` streams each file into `<name>.partial`, continues from there after an
    interruption, and verifies every file. A prepare step then converts the weights to f16 and keeps only CLIP's vision tower, which is why the
    packs are smaller on disk than in transit.
  - An `installed` marker holding the `ModelId` is written last, so a half-finished install never counts. After
    that, everything runs offline.
  - Uninstalling deletes the pack folder.
  - Models live in the app's data folder (`…/FinderMaxxing/models`), not in a cache, because they shouldn't be
    purged.
  - We don't self-host the files: the pinned revision plus the hash already make the source immaterial.
- **Inference:**
  - Models load on first use and then stay loaded.
  - All work runs on its own rayon pool with half the cores, entered from tokio's blocking pool. Indexing
    therefore never takes the whole machine.
  - CPU only for now.
- **The text model (e5):** "query: " and "passage: " prefixes, mean pooling, L2 normalisation, passages in
  batches of 8.
- **The image models (CLIP):** images are resized to 224 with CLIP's normalisation. Queries use multilingual
  DistilBERT (its attention mask is inverted: 1 = padding) and the dense projection.
- **Relevance floors,** measured with the real models (`tests/real_models.rs`):
  - e5: cross-lingual matches score 0.82–0.86, while unrelated documents reach 0.81, so the floor is 0.79. It
    only trims noise; ranking does the rest.
  - CLIP: matches score 0.26–0.31 and others ≤ 0.23, so the floor is 0.22.
- **Tests:**
  - Use-case tests run against bag-of-words fakes.
  - The adapter has a unit test of downloads, resume and hash failures against a local HTTP server.
  - `real_models` (ignored by default, nightly in CI with the models cached) downloads the packs and checks
    cross-lingual retrieval and text-to-photo search.

## Consequences
- **Indexing speed:** a few documents per second on a laptop CPU, by design rather than by limit. Metal/CUDA
  backends are a feature flag away in candle but untested.
- **Licences:** cargo-deny passes. Two transitive crates are unmaintained but not vulnerable, and are documented
  in `deny.toml`:
  - `paste`: a build-time macro used by gemm and tokenizers;
  - `ttf-parser`: used by lopdf for PDF text extraction.
- **Changing a model** means a new revision pin. The `ModelId` changes with it, and the index rebuilds itself.
