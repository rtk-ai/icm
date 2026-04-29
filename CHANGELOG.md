# Changelog

## [Unreleased]

### Added
- Jina AI embeddings v5 text backends (`jina-v5-nano`, `jina-v5-small`) via local ONNX inference
- Matryoshka representation learning: configurable truncate_dim for jina-v5 backends
- Asymmetric retrieval: `retrieval.query:` / `retrieval.passage:` instruction prefixes
- Auto re-embed on embedder dimension change (`--no-auto-reembed` to opt out)
- `embed_query` / `embed_document` distinction on the `Embedder` trait
- `icm config show` now displays active backend name and license tag
- `icm recall` now shows active model name in output header

### License note
Jina v5 model weights are CC BY-NC 4.0 (non-commercial). Commercial use requires a license from Jina AI.
