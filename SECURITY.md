# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in ICM, please report it to the maintainers privately:

- **Email**: security@rtk-ai.app (or open a private security advisory on GitHub)
- **Response time**: we aim to acknowledge reports within 48 hours
- **Disclosure**: we follow responsible disclosure practices (90-day embargo)

**Please do NOT:**
- Open public GitHub issues for security vulnerabilities
- Disclose vulnerabilities on social media or forums before we've had a chance to address them

---

## Security Review Process for Pull Requests

ICM parses untrusted input (tool output and transcripts via hooks, MCP tool arguments), runs SQL against a local database, can spawn a configured LLM CLI, and ships pre-built binaries. PRs from external contributors undergo enhanced review to protect against:

- **Injection** — SQL injection, or command injection via a spawned CLI
- **Untrusted-input handling** — malformed hook payloads / MCP arguments causing crashes, path traversal, or store poisoning
- **Supply chain attacks** — malicious or backdoored dependencies
- **Backdoors & data leaks** — logic bombs, exfiltration of the local memory store
- **Release tampering** — CI/CD or installer changes that alter published artifacts

---

## Automated Security Checks

Every PR runs the `security scan` job in [`ci.yml`](.github/workflows/ci.yml):

1. **Dependency audit** (`cargo audit`) — detects known CVEs
2. **New-dependency alert** — flags any dependency added to a `Cargo.toml` for supply-chain review
3. **Lints as gates** — `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` must pass

Maintainers can additionally run the `/security-review` skill on a branch for a deeper audit.

---

## Critical Files Requiring Enhanced Review

The following are **high-risk** and warrant extra scrutiny (and ideally a second reviewer):

### Tier 1: Untrusted input & execution
- **`crates/icm-store/src/**`** — all SQL and the SQLite schema/migrations (injection, corruption, data loss)
- **`crates/icm-mcp/src/tools.rs`** — MCP tool argument parsing and validation (untrusted client input)
- **`crates/icm-cli/src/summarizer.rs`** — spawns the configured LLM CLI (e.g. `claude -p`); must stay isolated and never build a shell string from untrusted input
- **`crates/icm-cli/src/summarizer/api.rs`** — calls LLM APIs with a user-supplied key. The key must come from the environment only, travel in a header (never the URL) to the host in `base_url` and nowhere else (redirects are not followed; a vendor's default key variable is only sent to that vendor's host), and never reach an error message, a log line or `icm config`. The log half of that also depends on `http_client_log_allowed` in `main.rs`, which keeps the HTTP client's header dump out of the log on any `RUST_LOG`. `config.toml` is trusted input: whoever can write it (or set `ICM_CONFIG`) chooses `base_url` and which environment variable is sent there — a project-level config must never be allowed to set `[*.summarizer]`
- **Hook entry points in `crates/icm-cli/src/main.rs`** — parse untrusted tool output / transcripts from stdin; must never crash the calling agent

### Tier 2: Distribution
- **`install.sh`** / **`install.ps1`** — installer logic and checksum verification (tampering, downgrade)
- **`Cargo.toml` / `Cargo.lock`** — dependency manifests (typosquatting, backdoored crates)
- **`.github/workflows/*.yml`** — CI/CD and release pipelines (artifact tampering, secret exfiltration)

If your PR modifies any of these, expect a detailed manual review and possibly a slower merge.

---

## Dangerous Patterns We Check For

| Pattern | Risk |
|---------|------|
| String-formatted SQL instead of bound parameters | SQL injection |
| `Command::new("sh"/"bash")` or shell strings from input | Command injection |
| `.unwrap()` / `.expect()` / `panic!` in non-test code | DoS via panic on malformed input |
| `unsafe {` | Memory safety |
| Unexpected `reqwest::` / `std::net::` on a hot/hook path | Data exfiltration |
| Un-canonicalized paths from input | Path traversal |
| `SystemTime::now() > …` conditionals, base64/hex blobs | Logic bombs / obfuscation |

---

## Security Best Practices for Contributors

**❌ DON'T**
```rust
// SQL injection
conn.execute(&format!("SELECT * FROM memories WHERE topic = '{topic}'"), [])?;

// Panic on invalid input
let topic = args.get("topic").unwrap();

// Shell string from untrusted input
Command::new("sh").arg("-c").arg(format!("claude -p {prompt}")).spawn()?;
```

**✅ DO**
```rust
// Bound parameters
conn.execute("SELECT * FROM memories WHERE topic = ?1", params![topic])?;

// Graceful error handling
let topic = get_str(args, "topic").ok_or_else(|| /* typed error */)?;

// Argv, no shell; keep the child isolated
Command::new("claude").args(["-p", "--model", model]).spawn()?;
```

### Error Handling
- Use `thiserror` for typed library errors and `anyhow` with `.context()` at the CLI boundary.
- **Never** `.unwrap()` / `.expect()` / `panic!` in `crates/*/src` (tests are fine).
- Propagate with `?`.

### Input Validation
- Treat hook stdin, transcripts, and MCP arguments as untrusted: validate lengths, reject empties, cap sizes.
- Always bind SQL parameters; never string-format values into SQL.
- Canonicalize file paths before use; validate topic/identifier shapes.

---

## Dependency Security

New dependencies should meet:
- **Downloads**: a healthy count on crates.io (or a strong justification if low)
- **Maintainer**: verifiable GitHub profile and track record
- **License**: MIT or Apache-2.0 compatible
- **Activity**: maintained (recent commits)
- **No typosquatting**: verified against similar crate names

Red flags: brand-new crate with low downloads, anonymous maintainer, name suspiciously close to a popular crate, or a recent license change.

---

## Disclosure Timeline

1. **Day 0**: acknowledgment sent to reporter
2. **Day 7**: severity and impact assessed
3. **Day 14**: patch development begins
4. **Day 30**: patch released + CVE filed (if applicable)
5. **Day 90**: public disclosure (or earlier if the patch is deployed)

Critical vulnerabilities (remote code execution, data exfiltration, store corruption) may be fast-tracked.

---

## Security Tooling

- **`cargo audit`** — CVE scanning (runs in CI)
- **`cargo clippy -- -D warnings`** — lints for unsafe/panic-prone patterns
- **GitHub Code Scanning (CodeQL)** — static analysis on PRs

---

## Contact

- **Security issues**: security@rtk-ai.app
- **General questions**: [GitHub Discussions](../../discussions)
