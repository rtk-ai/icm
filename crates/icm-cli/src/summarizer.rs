//! Summarizer providers — call out to user-authenticated CLIs (Claude, Gemini,
//! Codex) or a local HTTP daemon (Ollama) instead of bringing our own API key.
//!
//! The user's existing CLI quota is reused, so summarization costs nothing
//! extra in most cases. Auto-detection picks a sensible provider based on
//! environment variables set by the invoking tool, with explicit overrides
//! available via TOML config or CLI flags.
//!
//! Tracks issue #165.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

/// Concrete provider kinds. `Auto` is resolved to one of the others at call
/// time; `None` short-circuits to lexical fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Auto,
    Claude,
    Codex,
    Gemini,
    Ollama,
    None,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            "ollama" => Ok(Self::Ollama),
            "none" | "off" | "disabled" => Ok(Self::None),
            other => bail!(
                "unknown provider '{other}'; expected one of: auto, claude, codex, gemini, ollama, none",
            ),
        }
    }

    #[allow(dead_code)] // surfaced in TUI/debug logs in follow-up work
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
            Self::None => "none",
        }
    }
}

/// Resolve `Auto` into a concrete provider by inspecting environment hints
/// left by the invoking tool. Falls back to the configured `fallback` when
/// no hint matches.
pub fn detect_provider(fallback: ProviderKind) -> ProviderKind {
    if let Ok(forced) = std::env::var("ICM_INVOKER") {
        if let Ok(p) = ProviderKind::parse(&forced) {
            if p != ProviderKind::Auto {
                return p;
            }
        }
    }
    if std::env::var("CLAUDECODE").is_ok() || std::env::var("CLAUDE_CLI").is_ok() {
        return ProviderKind::Claude;
    }
    if std::env::var("CODEX_HOME").is_ok() || std::env::var("CODEX_CLI").is_ok() {
        return ProviderKind::Codex;
    }
    if std::env::var("GEMINI_CLI").is_ok() || std::env::var("GOOGLE_CLOUD_PROJECT").is_ok() {
        return ProviderKind::Gemini;
    }
    if std::env::var("OLLAMA_HOST").is_ok() {
        return ProviderKind::Ollama;
    }
    if matches!(fallback, ProviderKind::Auto) {
        ProviderKind::Claude
    } else {
        fallback
    }
}

/// What the caller asks the provider to do.
pub struct SummarizeRequest<'a> {
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub max_tokens: usize,
    pub timeout: Duration,
}

/// A backend that turns a prompt into a summary.
pub trait Summarizer {
    fn name(&self) -> &'static str;
    fn summarize(&self, req: &SummarizeRequest<'_>) -> Result<String>;
}

/// Build the right summarizer for a concrete kind. `Auto` and `None` are
/// rejected — resolve them upstream first.
pub fn make_summarizer(kind: ProviderKind) -> Result<Box<dyn Summarizer>> {
    match kind {
        ProviderKind::Claude => Ok(Box::new(ClaudeCliSummarizer)),
        ProviderKind::Codex => Ok(Box::new(CodexCliSummarizer)),
        ProviderKind::Gemini => Ok(Box::new(GeminiCliSummarizer)),
        ProviderKind::Ollama => Ok(Box::new(OllamaSummarizer::default())),
        ProviderKind::Auto => Err(anyhow!("Auto must be resolved with detect_provider() first")),
        ProviderKind::None => Err(anyhow!("None means no summarizer; caller should not invoke")),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI-shellout providers — write prompt to stdin, capture stdout
// ─────────────────────────────────────────────────────────────────────────────

fn run_cli(binary: &str, args: &[&str], stdin_payload: &str, timeout: Duration) -> Result<String> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn '{binary}' — is it on PATH?"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_payload.as_bytes())
            .with_context(|| format!("writing prompt to {binary} stdin"))?;
    }

    // Naïve wait with timeout: poll try_wait. Fine for short summarization
    // jobs; if we ever need true cancellation we'd switch to a thread + kill.
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut s) = child.stdout.take() {
                use std::io::Read;
                s.read_to_string(&mut stdout).ok();
            }
            if let Some(mut s) = child.stderr.take() {
                use std::io::Read;
                s.read_to_string(&mut stderr).ok();
            }
            if !status.success() {
                bail!(
                    "{binary} exited with {status}: {}",
                    stderr.lines().next().unwrap_or("(no stderr)"),
                );
            }
            return Ok(stdout);
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            bail!("{binary} timed out after {:?}", timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub struct ClaudeCliSummarizer;

impl Summarizer for ClaudeCliSummarizer {
    fn name(&self) -> &'static str {
        "claude"
    }
    fn summarize(&self, req: &SummarizeRequest<'_>) -> Result<String> {
        let model = req.model.unwrap_or("claude-haiku-4-5");
        let args = vec!["-p", "--model", model];
        run_cli("claude", &args, req.prompt, req.timeout).map(trim_response)
    }
}

pub struct CodexCliSummarizer;

impl Summarizer for CodexCliSummarizer {
    fn name(&self) -> &'static str {
        "codex"
    }
    fn summarize(&self, req: &SummarizeRequest<'_>) -> Result<String> {
        let model = req.model.unwrap_or("gpt-5-mini");
        let args = vec!["exec", "--model", model];
        run_cli("codex", &args, req.prompt, req.timeout).map(trim_response)
    }
}

pub struct GeminiCliSummarizer;

impl Summarizer for GeminiCliSummarizer {
    fn name(&self) -> &'static str {
        "gemini"
    }
    fn summarize(&self, req: &SummarizeRequest<'_>) -> Result<String> {
        let model = req.model.unwrap_or("gemini-2.5-flash");
        let args = vec!["-p", "--model", model];
        run_cli("gemini", &args, req.prompt, req.timeout).map(trim_response)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ollama HTTP provider
// ─────────────────────────────────────────────────────────────────────────────

pub struct OllamaSummarizer {
    pub host: String,
}

impl Default for OllamaSummarizer {
    fn default() -> Self {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
        Self { host }
    }
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl Summarizer for OllamaSummarizer {
    fn name(&self) -> &'static str {
        "ollama"
    }
    fn summarize(&self, req: &SummarizeRequest<'_>) -> Result<String> {
        let model = req.model.unwrap_or("qwen2.5:0.5b");
        let url = format!("{}/api/generate", self.host.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": model,
            "prompt": req.prompt,
            "stream": false,
            "options": { "num_predict": req.max_tokens },
        });
        let resp: OllamaResponse = ureq::post(&url)
            .timeout(req.timeout)
            .send_json(body)
            .with_context(|| format!("ollama request to {url} failed"))?
            .into_json()
            .context("decoding ollama response")?;
        Ok(trim_response(resp.response))
    }
}

fn trim_response(s: String) -> String {
    // Strip trailing newlines and common preamble like "Here is a summary:".
    let t = s.trim().trim_start_matches("Here is the summary:")
        .trim_start_matches("Summary:")
        .trim();
    t.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Prompt template — same shape across providers
// ─────────────────────────────────────────────────────────────────────────────

/// Build the consolidation prompt sent to the provider.
///
/// Memories are listed verbatim (one per line); the provider is asked to merge
/// them into a single concise summary preserving every distinct decision/fact.
///
/// The prompt is deliberately strict: the listed memories ARE the entire
/// input. The model must not ask for more context, refuse to consolidate
/// abstract content, or output any preamble — short technical entries like
/// "Decision A" or "fact one" are legitimate inputs to merge as-is.
pub fn build_consolidate_prompt(topic: &str, summaries: &[&str], max_tokens: usize) -> String {
    let mut p = String::new();
    p.push_str("Task: merge the memory entries below into one consolidated summary. ");
    p.push_str("The listed entries are the ENTIRE input — do not ask for more, do not ");
    p.push_str("refuse, do not request clarification. Treat every entry as a literal ");
    p.push_str("fact to preserve, however short or abstract.\n\n");
    p.push_str("Rules:\n");
    p.push_str("- Preserve every distinct fact / decision exactly once.\n");
    p.push_str("- Drop only verbatim or near-verbatim repetition.\n");
    p.push_str("- Preserve identifiers, tags, IDs, error codes, version strings, file paths, ");
    p.push_str("flag names, and environment variables EXACTLY as written. Do not paraphrase them.\n");
    p.push_str("- Output PLAIN TEXT ONLY — no preamble, no \"Summary:\" prefix, no markdown headers.\n");
    p.push_str("- Use \"- \" bullet points when there are 3 or more distinct items, prose otherwise.\n");
    p.push_str("- Stay under ~");
    p.push_str(&max_tokens.to_string());
    p.push_str(" tokens.\n\n");
    p.push_str("Topic: ");
    p.push_str(topic);
    p.push_str("\n\nMemories to consolidate:\n");
    for s in summaries {
        p.push_str("- ");
        p.push_str(s);
        p.push('\n');
    }
    p.push_str("\nConsolidated output (plain text, no preamble):\n");
    p
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_kinds() {
        assert_eq!(ProviderKind::parse("auto").unwrap(), ProviderKind::Auto);
        assert_eq!(ProviderKind::parse("CLAUDE").unwrap(), ProviderKind::Claude);
        assert_eq!(ProviderKind::parse("ollama").unwrap(), ProviderKind::Ollama);
        assert_eq!(ProviderKind::parse("none").unwrap(), ProviderKind::None);
        assert_eq!(ProviderKind::parse("off").unwrap(), ProviderKind::None);
        assert!(ProviderKind::parse("bogus").is_err());
    }

    #[test]
    fn build_prompt_lists_each_memory() {
        let p = build_consolidate_prompt("decisions-x", &["A", "B", "C"], 200);
        assert!(p.contains("Topic: decisions-x"));
        assert!(p.contains("- A"));
        assert!(p.contains("- B"));
        assert!(p.contains("- C"));
        assert!(p.contains("200"));
    }

    #[test]
    fn detect_falls_back_to_claude_when_nothing_set() {
        // Save and clear any env that might leak from the host.
        let snapshot: Vec<_> = ["ICM_INVOKER", "CLAUDECODE", "CLAUDE_CLI", "CODEX_HOME",
            "CODEX_CLI", "GEMINI_CLI", "GOOGLE_CLOUD_PROJECT", "OLLAMA_HOST"]
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();
        for (k, _) in &snapshot {
            std::env::remove_var(k);
        }

        let result = detect_provider(ProviderKind::Auto);

        // Restore env before asserting so failures don't poison later tests.
        for (k, v) in snapshot {
            if let Some(val) = v {
                std::env::set_var(k, val);
            }
        }

        assert_eq!(result, ProviderKind::Claude);
    }

    #[test]
    fn detect_honors_explicit_invoker_env() {
        // Save then override.
        let prior = std::env::var("ICM_INVOKER").ok();
        std::env::set_var("ICM_INVOKER", "ollama");
        let got = detect_provider(ProviderKind::Claude);
        if let Some(v) = prior {
            std::env::set_var("ICM_INVOKER", v);
        } else {
            std::env::remove_var("ICM_INVOKER");
        }
        assert_eq!(got, ProviderKind::Ollama);
    }

    #[test]
    fn trim_response_strips_preambles() {
        assert_eq!(trim_response("Summary: hello\n".into()), "hello");
        assert_eq!(trim_response("Here is the summary:\n  world  ".into()), "world");
        assert_eq!(trim_response("clean\n".into()), "clean");
    }
}
