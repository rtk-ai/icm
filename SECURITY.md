# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in ICM, report it privately:

- **Email**: security@rtk-ai.app, or create a private security advisory on GitHub
- **Acknowledgment target**: 48 hours
- **Disclosure**: responsible disclosure with a 90-day embargo by default

Please do not:

- Open public GitHub issues for security vulnerabilities
- Disclose vulnerabilities publicly before maintainers have had time to assess
  and patch them
- Include real user secrets, private memories, or private transcripts in public
  reports

When reporting, include:

- ICM version or commit SHA
- Operating system
- Installation method
- Reproduction steps
- Impact assessment
- Relevant logs with secrets and private memory content removed

---

## Supported Versions

ICM is pre-1.0 and under active development. Security fixes are normally applied
to the latest release and the active development branch. If you need support for
an older version, include the version and deployment constraints in your report.

---

## Security Review Process for Pull Requests

ICM is a local-first CLI and MCP tool that reads and writes user-controlled
memory, transcript, config, and integration files. PRs from external
contributors may receive enhanced security review for:

- Command execution and shell invocation
- Agent hook behavior
- MCP tool input handling
- Config file generation and mutation
- Path traversal and unsafe filesystem writes
- Memory, transcript, and database privacy
- Dependency and release-chain risk
- Unexpected network access

---

## Automated Security Checks

Every PR runs CI checks in [`.github/workflows/ci.yml`](.github/workflows/ci.yml):

1. **Formatting**: `cargo fmt --all -- --check`
2. **Clippy**: `cargo clippy --workspace --all-targets -- -D warnings`
3. **Tests**: `cargo test --workspace` on Linux, Windows, and macOS
4. **Dependency audit**: `cargo audit`
5. **New dependency check**: flags changes to `Cargo.toml` for manual review

Results are visible in GitHub Actions.

---

## Critical Areas Requiring Enhanced Review

Changes in these areas may require deeper maintainer review.

### Tier 1: Hooks, Config, and Command Execution

- `crates/icm-cli/src/main.rs`
- `crates/icm-cli/src/uninstall/`
- `crates/icm-cli/src/archive.rs`
- `crates/icm-cli/src/extract.rs`
- `crates/icm-cli/src/summarizer.rs`
- Integration setup that writes `CLAUDE.md`, `AGENTS.md`, `.mcp.json`,
  `settings.json`, MCP config, hooks, or plugin files

### Tier 2: MCP, HTTP, and Input Boundaries

- `crates/icm-mcp/src/server.rs`
- `crates/icm-cli/src/http_api.rs`
- JSON-RPC request parsing
- HTTP auth/token behavior
- Any path accepting untrusted tool, agent, or user input

### Tier 3: Memory, Transcript, and Storage Privacy

- `crates/icm-store/src/`
- Transcript recording and search
- Memory import/export, archive, sync, or backup behavior
- Database path selection and deletion logic

### Tier 4: Supply Chain and Release

- `Cargo.toml`
- `Cargo.lock`
- `.github/workflows/*.yml`
- `install.sh`
- `install.ps1`
- Release packaging and Homebrew tap update logic

If your PR modifies these areas, explain the security impact in the PR body.

---

## Review Checklist

Maintainers and reviewers should check:

- [ ] PR description matches actual changes
- [ ] No public logging of secrets, private memories, transcripts, or database
      contents
- [ ] File writes stay within intended config or data paths
- [ ] Destructive operations have dry-run or confirmation behavior where
      appropriate
- [ ] MCP and HTTP inputs validate required fields and fail safely
- [ ] New network behavior is explicit, documented, and optional where possible
- [ ] New dependencies are justified and reviewed for maintenance, license, and
      typosquatting risk
- [ ] Error handling avoids panics on user-controlled input
- [ ] Tests cover security-sensitive edge cases

---

## Dangerous Patterns We Check For

| Pattern | Risk |
|---------|------|
| Shell invocation with user-controlled strings | Command injection |
| Unvalidated filesystem paths | Path traversal or unintended overwrite/delete |
| Public logging of memory/transcript content | Privacy leak |
| Hardcoded tokens, URLs with credentials, or API keys | Secret exposure |
| New network calls in local-first paths | Data exfiltration or privacy regression |
| `unsafe` blocks | Memory safety risk |
| `.unwrap()` / `.expect()` on user-controlled input | Denial of service via panic |
| Broad recursive deletion | Data loss |
| CI workflow changes with elevated permissions | Release or secret exfiltration risk |

---

## Dependency Security

New dependencies must be justified in the PR. Reviewers should check:

- License compatibility with Apache-2.0
- Maintainer reputation and project history
- Recent activity
- Download count or adoption signal
- Typosquatting risk
- Whether existing workspace dependencies can already solve the problem

Avoid dependencies in hot paths unless they are needed and measured.

---

## Security Best Practices for Contributors

### Command Execution

Prefer direct binary execution with structured args over shell strings.

```rust
// Avoid: shell parses user-controlled text.
std::process::Command::new("sh")
    .arg("-c")
    .arg(user_input)
    .output()?;

// Prefer: no shell interpolation.
std::process::Command::new("icm")
    .arg("recall")
    .arg(user_input)
    .output()?;
```

### Error Handling

Use `Result` and context-rich errors for fallible operations. Avoid panics on
user-controlled input.

```rust
let path = std::env::args()
    .nth(1)
    .ok_or_else(|| anyhow::anyhow!("missing path argument"))?;
```

### File Writes

When writing config or generated files:

- Resolve the expected target path explicitly
- Avoid following untrusted path fragments
- Preserve unrelated user content
- Prefer dry-run or check modes for destructive flows
- Add tests with temporary directories

### Privacy

Treat memories, transcripts, database files, and agent config as private user
data. Do not print or upload them unless the user explicitly requests it.

---

## Disclosure Timeline

Default vulnerability handling timeline:

1. **Day 0**: Acknowledgment sent to reporter
2. **Day 7**: Maintainers assess severity and impact
3. **Day 14**: Patch development begins
4. **Day 30**: Patch released when practical
5. **Day 90**: Public disclosure, or earlier if a patch is available and
   coordinated disclosure is complete

Critical vulnerabilities may be fast-tracked.

---

## Security Tooling

- **`cargo audit`**: known Rust dependency CVEs
- **`cargo clippy`**: Rust linting and suspicious patterns
- **GitHub Actions**: CI gates and dependency change summaries
- **GitHub Dependabot**: dependency updates where configured

---

## Contact

- **Security issues**: security@rtk-ai.app
- **General questions**: https://github.com/rtk-ai/icm/issues
- **Repository**: https://github.com/rtk-ai/icm

Last updated: 2026-07-02
