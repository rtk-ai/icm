# hf-hub 0.5.0, patched for ICM

This is hf-hub 0.5.0 as published on crates.io (Apache-2.0, see LICENSE),
pulled in by fastembed 7 to download embedding models. The workspace uses it
through `[patch.crates-io]` in the root `Cargo.toml`. No model is stored here:
models are still downloaded from Hugging Face on first use.

Changes, all in `src/api/sync.rs` and marked `ICM patch (issue #507)`:

1. `Api::metadata`: when the CDN answers the `Range: bytes=0-0` size probe
   with a plain `200` and no `Content-Range` (seen with Hugging Face's Xet
   CDN), the size is taken from the Hub's `X-Linked-Size` header instead of
   failing with `MissingHeader("Content-Range")`.
2. `Api::download_from`: when resuming a partial download and the server
   ignores `Range` (`200` with the whole file), the partial file is truncated
   and the download starts over, instead of appending the whole file after
   the partial bytes.

Repository tooling and the example were left out. Everything else is
unchanged (`git log -- vendor/hf-hub` shows the pristine import first).

Tests: `crates/icm-core/tests/hf_hub_xet.rs` (a local stand-in for the Hub
and the CDN; one opt-in test against the real Hub with `--ignored`).

Remove this directory and the `[patch.crates-io]` entry once fastembed
depends on hf-hub 1.x, which reads `X-Linked-Size` itself.
