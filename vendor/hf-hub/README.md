This crates aims to emulate and be compatible with the
[huggingface_hub](https://github.com/huggingface/huggingface_hub/) python package.

compatible means the Api should reuse the same files skipping downloads if
they are already present and whenever this crate downloads or modifies this cache
it should be consistent with [huggingface_hub](https://github.com/huggingface/huggingface_hub/)

At this time only a limited subset of the functionality is present, the goal is to add new
features over time. We are currently treating this as an internal/external tool, meaning
we are currently modifying everything at will for our internal needs. This will eventually
stabilize as it matures to accomodate most of our needs.

If you're interested in using this, you're welcome to do it but be warned about potential changing grounds.

If you want to contribute, you are more than welcome.

However allowing new features or creating new features might be denied by lack of maintainability
time. We're focusing on what we currently internally need. Hopefully that subset is already interesting
to more users.


# How to use 

Add the dependency

```bash
cargo add hf-hub  # --features tokio
```
`tokio` feature will enable an async (and potentially faster) API.

Use the crate:

```rust
use hf_hub::api::sync::Api;

let api = Api::new().unwrap();

let repo = api.model("bert-base-uncased".to_string());
let _filename = repo.get("config.json").unwrap();

// filename  is now the local location within hf cache of the config.json file
```

# SSL/TLS

This library uses tokio default TLS implementations which is `native-tls` (openssl) for `tokio`.

If you want control over the TLS backend you can remove the default features and only add the backend you are intending to use. 

```bash
cargo add hf-hub --no-default-features --features ureq,rustls-tls
cargo add hf-hub --no-default-features --features ureq,native-tls
cargo add hf-hub --no-default-features --features tokio,rustls-tls
cargo add hf-hub --no-default-features --features tokio,native-tls
```


When using the [`ureq`](https://github.com/algesten/ureq) feature, you will always use its default TLS backend which is [rustls](https://github.com/rustls/rustls).

When using [`tokio`](https://github.com/tokio-rs/tokio), by default `default-tls` will be enabled, which means OpenSSL. If you want/need to use rustls, disable the default features and use `rustls-tls` in conjunction with `tokio`.
