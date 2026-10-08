//! API-key summarizer providers — the "bring your own token" half of #165.
//!
//! Three wire formats share one plumbing: Anthropic Messages, OpenAI Chat
//! Completions (also what OpenRouter, Mistral, Groq, vLLM, LM Studio… speak,
//! reached through `base_url`), and Google Gemini `generateContent`.
//!
//! Key handling, all enforced in this file:
//! - the key is read from an environment variable at call time; the config
//!   file only ever holds the variable's NAME;
//! - a vendor's conventional variable (`OPENAI_API_KEY`…) is only ever sent
//!   to that vendor's own host. Any other `base_url` gets a key only when
//!   `api_key_env` names one explicitly;
//! - it travels in a request header, never in the URL (URLs end up in
//!   transport errors and proxy logs), and only to the host in `base_url`:
//!   redirects are never followed;
//! - no error built here, and no `icm config` line, may contain it: every
//!   error leaving [`ApiSummarizer::summarize_with_key`] is redacted as a
//!   whole, server-supplied text is redacted before it is clipped, and a 401
//!   body is not quoted at all (some vendors echo a partially masked key).
//!
//! The HTTP client's own debug log prints request headers; `main` filters
//! it out (see `http_client_log_allowed`), whatever `RUST_LOG` says.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use super::{ProviderKind, SummarizeRequest, Summarizer, trim_response};

/// Anthropic API version header. Dated, but it is the current (and only)
/// stable value; newer behavior is opted into per feature, not by bumping it.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Pauses before the second and third attempt when the server gives no
/// usable `retry-after`. Two retries, like the vendors' own SDKs.
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

/// Smallest output cap sent to a vendor's own API. Reasoning models spend "a
/// few hundred to tens of thousands" of tokens before the first visible one;
/// the cap only bounds a runaway, unused headroom is not billed.
const VENDOR_OUTPUT_FLOOR: usize = 8192;

/// Smallest output cap sent anywhere else. A compatible server checks
/// prompt + cap against a context window this code cannot know — 8192 alone
/// overflows an 8K model (vLLM, TGI) on every request.
const COMPAT_OUTPUT_FLOOR: usize = 2048;

/// Largest `max_tokens` every current Anthropic model accepts (Haiku 4.5's
/// output limit; the larger models take 128K). Above it the API answers 400.
const ANTHROPIC_OUTPUT_MAX: usize = 64_000;

/// ureq's own default; kept as the upper bound of the connect phase.
const CONNECT_TIMEOUT_MAX: Duration = Duration::from_secs(30);

/// Settings only the API-key providers read, taken from `[*.summarizer]` in
/// config.toml. Empty means "use the provider's default".
#[derive(Debug, Clone, Default)]
pub struct ApiOptions {
    /// NAME of the environment variable that holds the key.
    pub api_key_env: String,
    /// Endpoint override (gateway, proxy, or an OpenAI-compatible server).
    pub base_url: String,
    /// Anthropic only: workspace the request runs in, required by keys that
    /// are not scoped to a single workspace. Not a secret.
    pub workspace_id: String,
}

/// Which wire format a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Anthropic,
    OpenAi,
    Google,
}

impl Flavor {
    fn from_kind(kind: ProviderKind) -> Option<Self> {
        match kind {
            ProviderKind::Anthropic => Some(Self::Anthropic),
            ProviderKind::OpenAi => Some(Self::OpenAi),
            ProviderKind::Google => Some(Self::Google),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
        }
    }

    fn default_key_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Google => "GEMINI_API_KEY",
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Google => "https://generativelanguage.googleapis.com/v1beta",
        }
    }

    /// Is `host` (lowercase, no port) the vendor's own API? OpenAI also
    /// serves data-residency projects from regional prefixes
    /// (`eu.api.openai.com`, `us.…`), which are the same API, catalog and
    /// keys. The leading dot keeps `evilapi.openai.com` out.
    fn is_vendor_host(self, host: &str) -> bool {
        match self {
            Self::Anthropic => host == "api.anthropic.com",
            Self::OpenAi => host == "api.openai.com" || host.ends_with(".api.openai.com"),
            Self::Google => host == "generativelanguage.googleapis.com",
        }
    }

    /// The vendor's current low-cost general model (not deprecated), used
    /// when none is configured — same policy as the CLI providers. Not
    /// always the cheapest ID on the price list: older or deprecated models
    /// can cost less. Checked against each vendor's published model list on
    /// 2026-10-04; model IDs age, so this is the first thing to revisit when
    /// a default starts returning 404.
    fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-haiku-4-5",
            Self::OpenAi => "gpt-6-luna",
            Self::Google => "gemini-3.5-flash-lite",
        }
    }
}

/// What was sent, for the messages that explain what came back.
struct Sent<'a> {
    model: &'a str,
    /// The model is ICM's built-in default, not something the user wrote.
    model_is_default: bool,
    /// `max_tokens` as configured.
    budget: usize,
    /// The output cap that went on the wire.
    ceiling: usize,
}

/// One summarizer for the three API-key providers.
pub struct ApiSummarizer {
    flavor: Flavor,
    /// No trailing slash.
    base_url: String,
    /// `base_url` is not the provider's default URL. Drives the "check
    /// `base_url`" hints only.
    custom_base: bool,
    /// The host is the vendor's own (default or regional). Decides what a
    /// third-party server must not inherit: the vendor's default key
    /// variable, default model and OpenAI-only request fields.
    vendor_host: bool,
    /// `http://` to a host that is not loopback: key and text go in clear.
    cleartext: bool,
    /// Variable the key is read from; `None` = no key is sent (third-party
    /// host and no `api_key_env`).
    key_env: Option<String>,
    workspace_id: Option<String>,
    backoff: [Duration; 2],
}

impl ApiSummarizer {
    pub fn new(kind: ProviderKind, opts: &ApiOptions) -> Result<Self> {
        let flavor = Flavor::from_kind(kind)
            .ok_or_else(|| anyhow!("'{}' is not an API-key provider", kind.as_str()))?;
        let endpoint = resolve_base_url(flavor, &opts.base_url)?;
        let key_env = resolve_key_env(flavor, &opts.api_key_env, endpoint.vendor_host)?;
        let workspace_id = resolve_workspace_id(&opts.workspace_id)?;
        Ok(Self {
            flavor,
            base_url: endpoint.url,
            custom_base: endpoint.custom,
            vendor_host: endpoint.vendor_host,
            cleartext: endpoint.cleartext,
            key_env,
            workspace_id,
            backoff: RETRY_BACKOFF,
        })
    }

    /// Read the key from the environment; empty = this endpoint gets no key.
    /// The error names the variable to set; nothing here ever returns or
    /// prints the value on failure.
    fn load_key(&self) -> Result<String> {
        let name = self.flavor.name();
        let Some(env) = &self.key_env else {
            return Ok(String::new());
        };
        let raw = std::env::var(env).unwrap_or_default();
        let key = raw.trim();
        if key.is_empty() {
            bail!(
                "{name} provider: no API key — set the {env} environment variable \
                 (`export {env}=<your key>`), or point `api_key_env` in the summarizer \
                 config at the variable that holds it. ICM reads the key from the \
                 environment only, never from config.toml"
            );
        }
        // An HTTP client rejects such a header value with an error that
        // quotes the whole header line — catch it here, without the value.
        if !key.bytes().all(|b| b.is_ascii_graphic()) {
            bail!(
                "{name} provider: the value of {env} contains whitespace or \
                 non-printable characters and cannot be sent as a header — re-export it"
            );
        }
        Ok(key.to_string())
    }

    /// The model to send, and whether it is ICM's built-in default rather
    /// than something the user wrote.
    fn resolve_model(&self, requested: Option<&str>) -> Result<(String, bool)> {
        let name = self.flavor.name();
        let requested = requested.map(str::trim).filter(|m| !m.is_empty());
        let model = match requested {
            Some(m) => m,
            // Same reasoning as the Ollama provider (#253): a compatible
            // server has its own model catalog, so silently sending OpenAI's
            // default there only produces a confusing 404 — or worse, runs
            // some other model the user never chose.
            None if self.flavor == Flavor::OpenAi && !self.vendor_host => bail!(
                "{name} provider with a custom `base_url` needs an explicit model — a \
                 compatible server has its own model catalog. Set `model = \"…\"` next \
                 to `base_url` in the summarizer config, or pass the model flag on \
                 the command line"
            ),
            None => self.flavor.default_model(),
        };
        let is_default = requested.is_none();
        if self.flavor == Flavor::Google {
            // The model goes into the URL path: accept the `models/…` form
            // the Gemini docs also use, and refuse anything that would
            // rewrite the path or smuggle a query string.
            let model = model.strip_prefix("models/").unwrap_or(model);
            let ok = !model.is_empty()
                && model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'));
            if !ok {
                bail!("{name} provider: '{model}' is not a valid Gemini model name");
            }
            return Ok((model.to_string(), is_default));
        }
        Ok((model.to_string(), is_default))
    }

    /// Name of the output-cap field for the OpenAI wire format. OpenAI
    /// deprecated `max_tokens` on this endpoint and its reasoning models
    /// reject it; compatible servers are the other way round — `max_tokens`
    /// is the field they all implement, `max_completion_tokens` only some.
    /// A server that rejects the guess gets the other one (see `call`).
    fn output_floor(&self) -> usize {
        if self.vendor_host {
            VENDOR_OUTPUT_FLOOR
        } else {
            COMPAT_OUTPUT_FLOOR
        }
    }

    /// Hard output cap sent on the wire.
    ///
    /// `max_tokens` in the config is an *approximate* budget: the prompt
    /// already asks the model to stay under it, and the CLI providers never
    /// enforce it at all. Sending it verbatim as the API cap would be wrong
    /// twice over: a summary a few tokens over budget would be cut
    /// mid-sentence, and reasoning/thinking tokens count against the cap on
    /// all three APIs, so a reasoning model would spend the whole budget
    /// before writing a word (the Ollama `<think>` failure of #253 again).
    /// The wire cap is therefore a runaway guard with headroom, and hitting
    /// it is reported as an error rather than passed off as a summary.
    ///
    /// Only Anthropic gets an upper bound: its limit is documented and
    /// exceeding it is a 400. The other two APIs, and whatever sits behind a
    /// `base_url`, have limits this code cannot know.
    fn output_ceiling(&self, max_tokens: usize) -> usize {
        let wanted = max_tokens.saturating_mul(4).max(self.output_floor());
        match self.flavor {
            Flavor::Anthropic => wanted.min(ANTHROPIC_OUTPUT_MAX),
            Flavor::OpenAi | Flavor::Google => wanted,
        }
    }

    fn openai_cap_field(&self) -> &'static str {
        if self.vendor_host {
            "max_completion_tokens"
        } else {
            "max_tokens"
        }
    }

    /// URL and JSON body for one request. `cap_field` is only read by the
    /// OpenAI format.
    fn build_request(
        &self,
        model: &str,
        prompt: &str,
        ceiling: usize,
        cap_field: &str,
    ) -> (String, Value) {
        let base = &self.base_url;
        match self.flavor {
            Flavor::Anthropic => (
                format!("{base}/v1/messages"),
                json!({
                    "model": model,
                    "max_tokens": ceiling,
                    "messages": [{ "role": "user", "content": prompt }],
                }),
            ),
            Flavor::OpenAi => {
                let mut body = json!({
                    "model": model,
                    "messages": [{ "role": "user", "content": prompt }],
                });
                body[cap_field] = json!(ceiling);
                // The default model reasons at `medium` unless told otherwise:
                // tokens billed and counted against the cap, for a task that
                // is merging text. Only sent for that exact model on OpenAI's
                // own hosts — other models reject `none`, and a compatible
                // server may reject the field altogether.
                if self.vendor_host && model == self.flavor.default_model() {
                    body["reasoning_effort"] = json!("none");
                }
                (format!("{base}/chat/completions"), body)
            }
            Flavor::Google => (
                format!("{base}/models/{model}:generateContent"),
                json!({
                    "contents": [{ "role": "user", "parts": [{ "text": prompt }] }],
                    "generationConfig": { "maxOutputTokens": ceiling },
                }),
            ),
        }
    }

    /// Everything after the key has been read from the environment. An
    /// empty `key` sends no credential. Whatever goes wrong, the error is
    /// redacted as a whole on the way out, so no call site below can leak
    /// the key by forgetting to.
    fn summarize_with_key(&self, key: &str, req: &SummarizeRequest<'_>) -> Result<String> {
        self.call(key, req)
            // Redacted, then made printable: the redaction decodes `\uXXXX`
            // escapes to find the key, which can turn a harmless escaped
            // ESC from the server back into a real one.
            .map_err(|e| anyhow!("{}", printable(&redact_secret(&e.to_string(), key))))
    }

    fn call(&self, key: &str, req: &SummarizeRequest<'_>) -> Result<String> {
        let name = self.flavor.name();
        let (model, model_is_default) = self.resolve_model(req.model)?;
        let mut ceiling = self.output_ceiling(req.max_tokens);
        let mut shrunk_ceiling = false;
        let mut cap_field = self.openai_cap_field();
        let mut swapped_cap_field = false;
        if self.cleartext {
            warn_cleartext_once(name, &self.base_url);
        }

        // One deadline for the whole call, retries and pauses included, so a
        // worker never runs longer than `timeout_secs` because of them.
        let deadline = Instant::now() + req.timeout;
        let mut retries = 0;
        let text = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.timeout_error(req.timeout));
            }
            let (url, body) = self.build_request(&model, req.prompt, ceiling, cap_field);
            let failed = match self.send_once(&url, &body, key, remaining, req.timeout) {
                Ok(text) => break text,
                Err(failed) => failed,
            };
            let sent = Sent {
                model: &model,
                model_is_default,
                budget: req.max_tokens,
                ceiling,
            };
            let error = self.http_or_transport_error(&failed, key, &sent);

            // A gateway in front of OpenAI's reasoning models wants the
            // other cap field. Decided on the structured error, once.
            if self.flavor == Flavor::OpenAi
                && !swapped_cap_field
                && failed.status == Some(400)
                && rejects_cap_field(&failed.body)
            {
                swapped_cap_field = true;
                cap_field = if cap_field == "max_tokens" {
                    "max_completion_tokens"
                } else {
                    "max_tokens"
                };
                continue;
            }

            // A compatible server whose context window cannot take the
            // floor: once, without it. This is the setting that lets a small
            // model be used at all — `max_tokens` then decides the cap.
            let without_floor = req.max_tokens.saturating_mul(4).max(1);
            if !self.vendor_host
                && !shrunk_ceiling
                && without_floor < ceiling
                && matches!(failed.status, Some(400 | 422))
                && mentions_token_limit(&failed.body)
                && !rejects_cap_field(&failed.body)
            {
                shrunk_ceiling = true;
                ceiling = without_floor;
                continue;
            }

            let Retry::After(hint) = failed.retry else {
                return Err(error);
            };
            let Some(pause) = self.backoff.get(retries).copied() else {
                return Err(error);
            };
            // A `retry-after` longer than what is left is an answer in
            // itself: fail now with the provider's message, don't sleep
            // through the budget.
            let pause = hint.unwrap_or(pause);
            if Instant::now() + pause >= deadline {
                return Err(error);
            }
            std::thread::sleep(pause);
            retries += 1;
        };

        let value: Value = serde_json::from_str(&text).map_err(|e| {
            anyhow!(
                "{name} returned a response that is not JSON ({e}){}",
                self.base_url_hint()
            )
        })?;
        // A failure reported inside a 200 (OpenRouter and other gateways do
        // this for an upstream outage or rate limit): the cause is in the
        // body, not in `base_url`.
        if let Some(reported) = self.error_in_success_body(&value, key) {
            bail!("{name} {reported} — a provider-side problem, retry later");
        }
        let reply = self.parse_reply(&value, key, req.prompt)?;
        let sent = Sent {
            model: &model,
            model_is_default,
            budget: req.max_tokens,
            ceiling,
        };
        self.finish(reply, &sent)
    }

    /// `Some(description)` when a 2xx body is an error report rather than
    /// an answer. `"error": null`, `""`, `{}` or `false` next to a real
    /// answer is not one.
    fn error_in_success_body(&self, v: &Value, key: &str) -> Option<String> {
        let present = match &v["error"] {
            Value::String(s) => !s.trim().is_empty(),
            Value::Object(o) => !o.is_empty(),
            _ => false,
        };
        // The other two formats only when the answer itself is missing:
        // their success bodies never carry an `error` member.
        let answer_missing = match self.flavor {
            Flavor::Anthropic => v.get("content").is_none(),
            Flavor::OpenAi => true,
            Flavor::Google => v.get("candidates").is_none() && v.get("promptFeedback").is_none(),
        };
        if !(present && answer_missing) {
            return None;
        }
        let detail = error_message(v).unwrap_or_default();
        Some(format!(
            "reported an error in a 200 response{}: {}",
            error_code(&v["error"], key),
            quote(&detail, key, 300),
        ))
    }

    /// One HTTP exchange. `Ok` is the body of a 2xx response.
    fn send_once(
        &self,
        url: &str,
        body: &Value,
        key: &str,
        remaining: Duration,
        configured_timeout: Duration,
    ) -> std::result::Result<String, Failed> {
        let name = self.flavor.name();
        // A dedicated agent per call, as `ureq::post` builds anyway, for two
        // settings the default one gets wrong here:
        // - redirects: ureq replays a redirected POST as a body-less GET
        //   (which can never succeed on these endpoints) and strips only
        //   `Authorization` — `x-api-key` / `x-goog-api-key` would follow to
        //   whatever host `Location` names;
        // - connect timeout: fixed at 30s by default, ignoring the request
        //   timeout, so `timeout_secs = 5` used to wait 30s on a dead host.
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(remaining.min(CONNECT_TIMEOUT_MAX))
            .build();
        let mut http = agent.post(url).timeout(remaining);
        match self.flavor {
            Flavor::Anthropic => {
                http = http.set("anthropic-version", ANTHROPIC_VERSION);
                if !key.is_empty() {
                    http = http.set("x-api-key", key);
                }
                if let Some(id) = &self.workspace_id {
                    http = http.set("anthropic-workspace-id", id);
                }
            }
            Flavor::OpenAi => {
                if !key.is_empty() {
                    http = http.set("authorization", &format!("Bearer {key}"));
                }
            }
            Flavor::Google => {
                if !key.is_empty() {
                    http = http.set("x-goog-api-key", key);
                }
            }
        }

        let started = Instant::now();
        match http.send_json(body) {
            Ok(resp) => {
                let status = resp.status();
                // With redirects off, ureq hands a 3xx back as a success.
                if (300..400).contains(&status) {
                    let (target, advice) = redirect_target(
                        resp.header("location").unwrap_or_default(),
                        &self.base_url,
                        key,
                    );
                    return Err(Failed::fatal(anyhow!(
                        "{name}: {} answered with a redirect (HTTP {status}) to {target} — \
                         ICM does not follow redirects on API calls, so the key is never \
                         sent to another host; {advice}",
                        self.base_url,
                    )));
                }
                resp.into_string().map_err(|e| {
                    Failed::fatal(if is_timeout(&e) {
                        self.timeout_error(configured_timeout)
                    } else {
                        anyhow!("{name}: reading the response failed: {e}")
                    })
                })
            }
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp.header("retry-after").and_then(parse_retry_after);
                let body = resp.into_string().unwrap_or_default();
                let retry = match code {
                    408 | 500..=599 => Retry::After(retry_after),
                    // A spent quota keeps failing; a rate limit clears.
                    429 if !quota_exhausted(&body) => Retry::After(retry_after),
                    _ => Retry::No,
                };
                Err(Failed {
                    error: None,
                    status: Some(code),
                    retry_after,
                    body,
                    retry,
                })
            }
            Err(ureq::Error::Transport(t)) => {
                let connecting = t.kind() == ureq::ErrorKind::ConnectionFailed
                    || t.kind() == ureq::ErrorKind::Dns;
                if is_timeout(&t) {
                    // A timeout has spent its budget, and the request may
                    // have been processed (and billed): never retried.
                    return Err(Failed::fatal(if connecting {
                        anyhow!(
                            "{name}: could not connect to {} within {:.1?} — the host is \
                             not answering; check the network (VPN, firewall) and `base_url`",
                            self.base_url,
                            started.elapsed(),
                        )
                    } else {
                        self.timeout_error(configured_timeout)
                    }));
                }
                let error = anyhow!(
                    "{name} request failed: {} — check network access{}",
                    redact(&t.to_string(), key),
                    self.base_url_hint(),
                );
                // Refused, DNS hiccup, connection dropped before any
                // response: nothing was processed, safe to try again.
                let retryable = connecting || t.kind() == ureq::ErrorKind::Io;
                Err(Failed {
                    error: Some(error),
                    status: None,
                    retry_after: None,
                    body: String::new(),
                    retry: if retryable {
                        Retry::After(None)
                    } else {
                        Retry::No
                    },
                })
            }
        }
    }

    fn http_or_transport_error(
        &self,
        failed: &Failed,
        key: &str,
        sent: &Sent<'_>,
    ) -> anyhow::Error {
        match (&failed.error, failed.status) {
            (Some(e), _) => anyhow!("{e}"),
            (None, Some(code)) => {
                self.http_error(code, failed.retry_after, &failed.body, key, sent)
            }
            (None, None) => anyhow!("{} request failed", self.flavor.name()),
        }
    }

    /// Pull the answer and the stop signal out of a 200 response.
    fn parse_reply(&self, v: &Value, key: &str, prompt: &str) -> Result<Reply> {
        let name = self.flavor.name();
        let shape = |what: &str| {
            anyhow!(
                "{name} returned an unexpected response ({what}){}",
                self.base_url_hint()
            )
        };
        match self.flavor {
            Flavor::Anthropic => {
                let blocks = v
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| shape("no `content` array"))?;
                // Only `text` blocks are the answer; `thinking` blocks (on
                // by default on the larger models) are not.
                let text: String = blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect();
                let stop = v["stop_reason"].as_str();
                // Allow-list: only a natural stop is a complete answer. Any
                // other value — `pause_turn`, `tool_use`, one Anthropic adds
                // later — and a missing `stop_reason` (a gateway that drops
                // the field cannot say the answer is whole) are not assumed
                // complete: accepting a cut-off answer deletes memories,
                // refusing a whole one deletes nothing.
                let outcome = match stop {
                    Some("end_turn" | "stop_sequence") => Outcome::Complete,
                    None => Outcome::Abnormal("no stop_reason".to_string()),
                    Some("max_tokens") => Outcome::OutputCap,
                    Some("model_context_window_exceeded") => Outcome::ContextWindow,
                    Some("refusal") => Outcome::Declined(
                        match v.pointer("/stop_details/category").and_then(Value::as_str) {
                            Some(cat) => {
                                format!("stop_reason=refusal, category={}", quote(cat, key, 60))
                            }
                            None => "stop_reason=refusal".to_string(),
                        },
                    ),
                    Some(other) => {
                        Outcome::Abnormal(format!("stop_reason={}", quote(other, key, 60)))
                    }
                };
                Ok(Reply {
                    text,
                    stop: stop.map(|s| quote(s, key, 60)),
                    outcome,
                })
            }
            Flavor::OpenAi => {
                let choice = v
                    .pointer("/choices/0")
                    .ok_or_else(|| shape("no `choices`"))?;
                let message = &choice["message"];
                let text = match &message["content"] {
                    Value::String(s) => s.clone(),
                    // Some compatible servers return typed content parts.
                    Value::Array(parts) => {
                        parts.iter().filter_map(|p| p["text"].as_str()).collect()
                    }
                    _ => String::new(),
                };
                let stop = choice["finish_reason"].as_str();
                let reason = stop.map(|s| s.trim().to_ascii_lowercase());
                // Allow-list, as for the other two formats: only a natural
                // stop is a complete answer. Compatible servers have their
                // own ways of saying "cut short" (`model_length` on Mistral,
                // `insufficient_system_resource` / `aborted` on DeepSeek…)
                // and will invent more; an answer whose reason is unknown or
                // missing is not stored as a summary. The names accepted are
                // the natural stops of OpenAI, TGI and the Anthropic-style
                // gateways.
                let mut outcome = match (message["refusal"].as_str(), reason.as_deref()) {
                    (Some(r), _) if !r.trim().is_empty() => {
                        Outcome::Declined(format!("refusal: {}", quote(r, key, 200)))
                    }
                    (_, Some("stop" | "eos" | "eos_token" | "end_turn" | "stop_sequence")) => {
                        Outcome::Complete
                    }
                    (_, Some("length" | "max_tokens")) => Outcome::OutputCap,
                    (_, Some("model_length")) => Outcome::ContextWindow,
                    (_, Some("content_filter")) => {
                        Outcome::Declined("finish_reason=content_filter".to_string())
                    }
                    (_, Some("error")) => {
                        Outcome::ProviderFailed("finish_reason=error".to_string())
                    }
                    (_, Some(other)) => {
                        Outcome::Abnormal(format!("finish_reason={}", quote(other, key, 60)))
                    }
                    (_, None) => Outcome::Abnormal("no finish_reason".to_string()),
                };
                let failed_mid_answer = match &choice["error"] {
                    Value::String(s) => !s.trim().is_empty(),
                    Value::Object(o) => !o.is_empty(),
                    _ => false,
                };
                if failed_mid_answer {
                    let detail = error_message(choice).unwrap_or_default();
                    outcome = Outcome::ProviderFailed(format!(
                        "it failed mid-answer{}: {}",
                        error_code(&choice["error"], key),
                        quote(&detail, key, 300),
                    ));
                }
                // Servers that do not separate reasoning from the answer
                // (Groq's default `reasoning_format`, vLLM without a
                // reasoning parser) put the model's scratchpad in `content`.
                // OpenAI's own API never does.
                let text =
                    if self.vendor_host {
                        text
                    } else {
                        match split_reasoning(&text, mentions_think_tag(prompt)) {
                            Reasoning::Answer(answer) => answer.to_string(),
                            verdict => {
                                if matches!(outcome, Outcome::Complete) {
                                    outcome = Outcome::Abnormal(match verdict {
                                    Reasoning::Unfinished => {
                                        "the answer is an unfinished <think> reasoning block"
                                    }
                                    _ => {
                                        "the answer contains <think> markup that may be \
                                         the model's reasoning or a quotation of the tag: \
                                         reasoning cannot be told from a quotation, so it \
                                         is not cut (have the server separate reasoning, \
                                         or use a model that does not emit it)"
                                    }
                                }
                                .to_string());
                                }
                                String::new()
                            }
                        }
                    };
                Ok(Reply {
                    text,
                    stop: stop.map(|s| quote(s, key, 60)),
                    outcome,
                })
            }
            Flavor::Google => {
                let blocked = v
                    .pointer("/promptFeedback/blockReason")
                    .and_then(Value::as_str)
                    .map(|r| format!("prompt blocked, blockReason={}", quote(r, key, 60)));
                let Some(candidate) = v.pointer("/candidates/0") else {
                    return match blocked {
                        Some(why) => Ok(Reply {
                            text: String::new(),
                            stop: None,
                            outcome: Outcome::Declined(why),
                        }),
                        None => Err(shape("no `candidates`")),
                    };
                };
                let text: String = candidate["content"]["parts"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter(|p| p["thought"] != true)
                            .filter_map(|p| p["text"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                let stop = candidate["finishReason"].as_str();
                // Allow-list: `FinishReason` has two dozen values and grows.
                // Only STOP is a complete answer; a filter verdict is a
                // refusal; everything else, a missing reason included, is an
                // answer that cannot be trusted to be whole.
                let outcome = match (blocked, stop) {
                    (Some(why), _) => Outcome::Declined(why),
                    (None, Some("STOP")) => Outcome::Complete,
                    (None, Some("MAX_TOKENS")) => Outcome::OutputCap,
                    (
                        None,
                        Some(
                            r @ ("SAFETY"
                            | "RECITATION"
                            | "BLOCKLIST"
                            | "PROHIBITED_CONTENT"
                            | "SPII"
                            | "LANGUAGE"
                            | "ESCALATION"
                            | "PUP_LIMITED_DISABLED"),
                        ),
                    ) => Outcome::Declined(format!("finishReason={r}")),
                    (None, Some(r)) if r.starts_with("IMAGE_") => {
                        Outcome::Declined(format!("finishReason={}", quote(r, key, 60)))
                    }
                    (None, other) => {
                        let reason = other.map_or("missing".to_string(), |r| quote(r, key, 60));
                        let detail = candidate["finishMessage"]
                            .as_str()
                            .map(|m| format!(": {}", quote(m, key, 200)))
                            .unwrap_or_default();
                        Outcome::Abnormal(format!("finishReason={reason}{detail}"))
                    }
                };
                Ok(Reply {
                    text,
                    stop: stop.map(|s| quote(s, key, 60)),
                    outcome,
                })
            }
        }
    }

    /// Turn a parsed reply into the summary, or into an error the caller's
    /// existing provider-failure path handles.
    ///
    /// An empty, cut-off or otherwise unfinished answer is an `Err`, not
    /// `Ok`: `extract-pending` drops queue rows on empty output and stores
    /// every returned line as a fact, and a consolidation built on half a
    /// summary would replace the originals with it.
    fn finish(&self, reply: Reply, sent: &Sent<'_>) -> Result<String> {
        let name = self.flavor.name();
        let (model, budget, ceiling) = (sent.model, sent.budget, sent.ceiling);
        let floor = self.output_floor();
        match reply.outcome {
            Outcome::Complete => {}
            Outcome::Declined(why) => bail!(
                "{name} declined to answer ({why}; model={model}) — the content tripped the \
                 provider's safety filter; try another model or provider for this input"
            ),
            Outcome::OutputCap => {
                let at_max = self.flavor == Flavor::Anthropic && ceiling == ANTHROPIC_OUTPUT_MAX;
                let advice = if at_max {
                    "that is the largest cap this provider accepts on every model: reduce \
                     the input or pick a non-reasoning model"
                        .to_string()
                } else {
                    format!(
                        "`max_tokens = {budget}` gives a ceiling of {ceiling} (4x, minimum \
                         {floor}): set `max_tokens` above {} in the summarizer config \
                         to raise it, or pick a non-reasoning model",
                        ceiling / 4,
                    )
                };
                bail!(
                    "{name} stopped at the {ceiling}-token output ceiling before finishing \
                     (model={model}), so the incomplete answer was discarded. \
                     Reasoning/thinking tokens count against that ceiling; {advice}"
                )
            }
            Outcome::ContextWindow => bail!(
                "{name} filled the model's context window before finishing (model={model}), \
                 so the incomplete answer was discarded. Raising `max_tokens` would not \
                 help: reduce the input (a smaller `--limit` for extract-pending) or pick \
                 a model with a larger context window"
            ),
            // The provider said it failed: the URL is not what to look at.
            Outcome::ProviderFailed(why) => bail!(
                "{name}: the provider reported a failure ({why}; model={model}), so the \
                 incomplete answer was discarded — a provider-side problem, retry later"
            ),
            Outcome::Abnormal(why) => bail!(
                "{name} stopped before finishing ({why}; model={model}), so the incomplete \
                 answer was discarded{}",
                self.base_url_hint(),
            ),
        }
        let text = trim_response(reply.text);
        if text.is_empty() {
            bail!(
                "{name} returned an empty response (model={model}, stop={}) — retry, or \
                 set another model with `model = \"…\"` in the summarizer config",
                reply.stop.as_deref().unwrap_or("unknown"),
            );
        }
        Ok(text)
    }

    /// Map a non-2xx status to a message the user can act on. `body` is
    /// server-controlled: it is only quoted after redaction, and not at all
    /// for 401.
    fn http_error(
        &self,
        code: u16,
        retry_after: Option<Duration>,
        body: &str,
        key: &str,
        sent: &Sent<'_>,
    ) -> anyhow::Error {
        let name = self.flavor.name();
        let (model, model_is_default) = (sent.model, sent.model_is_default);
        let env = self.key_env.as_deref().unwrap_or("api_key_env");
        let detail = error_detail(body, key);
        let hint = self.base_url_hint();
        match code {
            401 if key.is_empty() => anyhow!(
                "{name}: the server at {} requires a key (HTTP 401) and none was sent — \
                 set `api_key_env` in the summarizer config to the NAME of the environment \
                 variable holding it",
                self.base_url,
            ),
            401 => anyhow!(
                "{name} rejected the API key (HTTP 401) — check that {env} holds a valid, \
                 non-revoked key for this provider{}",
                if self.custom_base {
                    " and that `base_url` is the endpoint the key belongs to"
                } else {
                    ""
                },
            ),
            403 => anyhow!(
                "{name} refused the request (HTTP 403): {detail} — the key in {env} is not \
                 allowed to use model '{model}' (check its workspace/project, billing and region)"
            ),
            429 => {
                let retry = retry_after
                    .map(|d| format!(", retry-after={}s", d.as_secs()))
                    .unwrap_or_default();
                anyhow!(
                    "{name} rate limit or quota exceeded (HTTP 429{retry}): {detail} — retry \
                     later, or check the usage limits and billing of the key in {env}"
                )
            }
            400 if self.flavor == Flavor::Anthropic
                && detail.contains("anthropic-workspace-id") =>
            {
                anyhow!(
                    "{name} rejected the request (HTTP 400): {detail} — this key is not \
                     scoped to a single workspace: set `workspace_id = \"wrkspc_…\"` in the \
                     summarizer config, or create a key scoped to one workspace"
                )
            }
            400 | 422 if mentions_token_limit(body) => {
                let floor = self.output_floor();
                // On a vendor host the cap never goes under the floor, so
                // at the floor lowering `max_tokens` changes nothing on the
                // wire: do not send the user round in circles. Elsewhere the
                // floor gives way (see `call`), so lowering it does help.
                let advice = if self.vendor_host && sent.ceiling <= floor {
                    format!(
                        "the output cap sent ({}) is the smallest ICM sends to this \
                         provider, so lowering `max_tokens` (now {}) would not change it: \
                         shorten the input or pick a model with a larger context window or \
                         output limit",
                        sent.ceiling, sent.budget,
                    )
                } else {
                    format!(
                        "the value sent ({}) is ICM's output ceiling, derived from \
                         `max_tokens` (now {}): lower `max_tokens`, shorten the input, or \
                         pick a model with a larger context window or output limit",
                        sent.ceiling, sent.budget,
                    )
                };
                anyhow!(
                    "{name} rejected the request (HTTP {code}, model={model}): {detail} — {advice}{hint}"
                )
            }
            404 if self.workspace_id.is_some()
                && detail.to_ascii_lowercase().contains("workspace") =>
            {
                anyhow!(
                    "{name} returned HTTP 404: {detail} — check `workspace_id` in the \
                     summarizer config{hint}"
                )
            }
            404 if model_is_default => anyhow!(
                "{name} returned HTTP 404: {detail} — '{model}' is ICM's built-in default \
                 and this key cannot use it (retired, or not enabled for your \
                 organization/project); set `model = \"…\"` in the summarizer config or \
                 pass the model flag{hint}"
            ),
            404 => anyhow!(
                "{name} returned HTTP 404: {detail} — check the model name ('{model}') and \
                 that this key has access to it{hint}"
            ),
            408 | 500..=599 => anyhow!(
                "{name} is unavailable (HTTP {code}): {detail} — a provider-side problem, \
                 retry later"
            ),
            _ => {
                anyhow!("{name} rejected the request (HTTP {code}, model={model}): {detail}{hint}")
            }
        }
    }

    fn timeout_error(&self, timeout: Duration) -> anyhow::Error {
        anyhow!(
            "{} request timed out after {timeout:?} — raise `timeout_secs` in the summarizer \
             config, or check connectivity to {}",
            self.flavor.name(),
            self.base_url,
        )
    }

    /// Appended to errors that a wrong endpoint would explain.
    fn base_url_hint(&self) -> String {
        if self.custom_base {
            format!(" (check `base_url`, currently {})", self.base_url)
        } else {
            String::new()
        }
    }
}

impl Summarizer for ApiSummarizer {
    fn name(&self) -> &'static str {
        self.flavor.name()
    }
    fn summarize(&self, req: &SummarizeRequest<'_>) -> Result<String> {
        let key = self.load_key()?;
        self.summarize_with_key(&key, req)
    }
}

/// Stand-in for an API-key provider whose options do not validate: every
/// call returns the validation error, so the caller's provider-failed path
/// runs instead of the whole command aborting.
struct Misconfigured {
    name: &'static str,
    error: String,
}

impl Summarizer for Misconfigured {
    fn name(&self) -> &'static str {
        self.name
    }
    fn summarize(&self, _req: &SummarizeRequest<'_>) -> Result<String> {
        Err(anyhow!("{}", self.error))
    }
}

/// Build the summarizer for an API-key provider kind.
pub(super) fn make(kind: ProviderKind, opts: &ApiOptions) -> Box<dyn Summarizer> {
    match ApiSummarizer::new(kind, opts) {
        Ok(s) => Box::new(s),
        Err(e) => Box::new(Misconfigured {
            name: kind.as_str(),
            error: e.to_string(),
        }),
    }
}

/// What a 200 response boiled down to.
struct Reply {
    text: String,
    /// The provider's own stop/finish reason, for diagnostics. Already
    /// redacted and clipped.
    stop: Option<String>,
    outcome: Outcome,
}

/// Whether the answer in a 200 response can be used. The reasons carried
/// here are server text, already redacted and clipped.
enum Outcome {
    Complete,
    /// The output cap was hit: raising `max_tokens` helps.
    OutputCap,
    /// The model's context window filled up: only less input helps.
    ContextWindow,
    /// Refused or filtered by the provider.
    Declined(String),
    /// The provider itself says the generation failed.
    ProviderFailed(String),
    /// Stopped for any other reason, or for none we can tell.
    Abnormal(String),
}

/// A failed HTTP exchange, with what the retry loop needs to decide.
struct Failed {
    /// Ready-made error (transport failures, redirects). HTTP statuses are
    /// rendered later from `status` + `body`.
    error: Option<anyhow::Error>,
    status: Option<u16>,
    retry_after: Option<Duration>,
    body: String,
    retry: Retry,
}

impl Failed {
    fn fatal(error: anyhow::Error) -> Self {
        Self {
            error: Some(error),
            status: None,
            retry_after: None,
            body: String::new(),
            retry: Retry::No,
        }
    }
}

enum Retry {
    No,
    /// Worth another attempt, after the server's `retry-after` if it gave one.
    After(Option<Duration>),
}

/// `retry-after` as delta-seconds only. The HTTP-date form is ignored, and
/// so is anything else: the header is server text and must not be echoed.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() || value.len() > 6 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().map(Duration::from_secs)
}

/// A 429 that will not clear by waiting: spending cap or exhausted quota.
/// Anthropic sends those without `retry-after` and names them in
/// `error.details.error_code`; OpenAI uses `insufficient_quota`.
fn quota_exhausted(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let is =
        |pointer: &str, wanted: &str| v.pointer(pointer).and_then(Value::as_str) == Some(wanted);
    is("/error/details/error_code", "enforced_spend_limit_reached")
        || is("/error/code", "insufficient_quota")
        || is("/error/type", "insufficient_quota")
}

/// Does this 400 say the output-cap field itself is the problem? Read from
/// the structured error, not from a substring of the message.
fn rejects_cap_field(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let param = v.pointer("/error/param").and_then(Value::as_str);
    matches!(param, Some("max_tokens" | "max_completion_tokens"))
        || v.pointer("/error/code").and_then(Value::as_str) == Some("unsupported_parameter")
}

/// Does this 400 complain about the output cap or the context window? Read
/// from the message: no API has a structured field for it. Only used to
/// pick a better hint and to try once more with a smaller cap, never to
/// accept an answer.
fn mentions_token_limit(body: &str) -> bool {
    let text = body.to_ascii_lowercase();
    [
        "max_tokens",
        "max_completion_tokens",
        "maxoutputtokens",
        "max_new_tokens",
        "context length",
        "context window",
        "context size",
        "maximum context",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// What the text of an OpenAI-format answer turned out to be.
enum Reasoning<'a> {
    /// The answer, any leading scratchpad removed.
    Answer(&'a str),
    /// Nothing but a `<think>` block that never closes.
    Unfinished,
    /// It has the shape of a scratchpad, but the input talks about these
    /// tags, so it may just as well be the answer quoting them.
    Ambiguous,
}

/// Does this text talk about `<think>` tags, in any of the spellings they
/// reach a memory in (`</THINK>`, `&lt;/think&gt;`, a JSON `\u003c/think`,
/// "the closing think tag")?
fn mentions_think_tag(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "think>",
        "think&gt;",
        "think\\u003e",
        "think tag",
        "<think",
        "&lt;think",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Separate a leading reasoning scratchpad from the answer.
///
/// Only one shape is cut: a `<think>…</think>` block at the very start,
/// when nothing says it could be the answer itself — the input does not
/// talk about these tags, and no tag is left after the cut. Everything else
/// that looks like a scratchpad is refused rather than cut, because a
/// perfectly good answer looks the same when the memories being summarized
/// are *about* these tags (ICM's own notes on #253 are), and cutting there
/// stored half a summary and deleted the originals:
/// - a lone `</think>` with no opening tag before it — what chat templates
///   that pre-fill the opening tag produce, and also what quoting the tag
///   produces. It used to be cut whenever the input did not contain the
///   literal tag; a memory spelling it `&lt;/think&gt;` was enough to lose
///   everything before it;
/// - a leading block when the input mentions the tags, in any spelling.
///
/// A mention anywhere else in the text is left alone.
fn split_reasoning(text: &str, input_mentions_tag: bool) -> Reasoning<'_> {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let trimmed = text.trim_start();
    // Same byte offsets as `trimmed`: ASCII lowercasing keeps lengths.
    let lower = trimmed.to_ascii_lowercase();
    if let Some(after_open) = lower.strip_prefix(OPEN) {
        let Some(end) = after_open.find(CLOSE) else {
            return Reasoning::Unfinished;
        };
        let rest = &trimmed[OPEN.len() + end + CLOSE.len()..];
        if input_mentions_tag || mentions_think_tag(rest) {
            return Reasoning::Ambiguous;
        }
        return Reasoning::Answer(rest);
    }
    match (lower.find(CLOSE), lower.find(OPEN)) {
        (Some(close), open) if open.is_none_or(|o| o > close) => Reasoning::Ambiguous,
        _ => Reasoning::Answer(text),
    }
}

fn resolve_key_env(flavor: Flavor, configured: &str, vendor_host: bool) -> Result<Option<String>> {
    let name = configured.trim();
    if name.is_empty() {
        // The vendor's conventional variable belongs to the vendor's host.
        // A local or third-party server gets no key unless one is named:
        // otherwise `base_url = "http://gpu-box:8000/v1"` would ship the
        // real OPENAI_API_KEY of whoever has one exported.
        return Ok(vendor_host.then(|| flavor.default_key_env().to_string()));
    }
    // POSIX-portable names only. Besides catching typos, this is what stops
    // the classic mistake — pasting the key itself into `api_key_env` — from
    // leaking: real keys carry `-` or lowercase letters, so they land here
    // and the message below deliberately does not repeat the value.
    let mut bytes = name.bytes();
    let valid = bytes
        .next()
        .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if !valid {
        bail!(
            "`api_key_env` for the {} provider must be the NAME of an environment variable \
             (uppercase letters, digits and underscores, e.g. {}), not the key itself — \
             the configured value is not repeated here in case it is one",
            flavor.name(),
            flavor.default_key_env(),
        );
    }
    Ok(Some(name.to_string()))
}

/// The workspace id goes into a header: keep it to characters that cannot
/// break one. Its format is the server's business (it answers 400 on a bad
/// one), so no `wrkspc_` prefix is enforced here.
fn resolve_workspace_id(configured: &str) -> Result<Option<String>> {
    let id = configured.trim();
    if id.is_empty() {
        return Ok(None);
    }
    let valid = id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid {
        bail!(
            "`workspace_id` must be an Anthropic workspace id (letters, digits, `_` and \
             `-`, e.g. wrkspc_01AbC…)"
        );
    }
    Ok(Some(id.to_string()))
}

/// A validated `base_url`.
struct Endpoint {
    /// No trailing slash.
    url: String,
    /// Differs from the provider's default URL.
    custom: bool,
    vendor_host: bool,
    /// `http://` to a non-loopback host.
    cleartext: bool,
}

fn resolve_base_url(flavor: Flavor, configured: &str) -> Result<Endpoint> {
    let default = flavor.default_base_url();
    // Emptiness is decided before the trailing slashes go: a value made of
    // slashes only (a template like "${LLM_BASE_URL}/" rendered with an
    // empty variable) is malformed, not "use the vendor's endpoint" — that
    // would send the key meant for the gateway to the vendor instead.
    let configured = configured.trim();
    let url = configured.trim_end_matches('/');
    if configured.is_empty() {
        return Ok(Endpoint {
            url: default.to_string(),
            custom: false,
            vendor_host: true,
            cleartext: false,
        });
    }
    let name = flavor.name();
    // None of the refusals below repeats the value: the usual reason a
    // `base_url` is malformed is a credential pasted into it.
    //
    // A query string or fragment can only be a mistake — the API path is
    // appended after it — and the classic one is `?key=…` / `?api-key=…`,
    // which `icm config` and every transport error would then print.
    if url.contains(['?', '#']) {
        bail!(
            "`base_url` for the {name} provider must not carry a query string or a \
             fragment — the key goes in the environment variable named by `api_key_env`"
        );
    }
    let (plain_http, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (false, rest),
        (None, Some(rest)) => (true, rest),
        (None, None) => {
            bail!("`base_url` for the {name} provider must start with http:// or https://")
        }
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.contains('@') {
        bail!(
            "`base_url` for the {name} provider must not embed credentials \
             (user:password@host) — put the key in the environment variable named by \
             `api_key_env`"
        );
    }
    // The host decides whether the vendor's own key variable may be sent,
    // so it must be the host the HTTP client will really connect to. Two
    // guards, because that client's URL parser is lenient where this code
    // is not (`\` is a path separator to it, `%2e` is a dot):
    // 1. only a plain host name, IPv4, bracketed IPv6 and a numeric port
    //    are accepted — no `\`, `%`, space, control or non-ASCII character;
    // 2. the host is then compared with what the client's own parser gets.
    let host = host_of(authority).ok_or_else(|| {
        anyhow!(
            "`base_url` for the {name} provider has an invalid host or port — only \
             letters, digits, `.`, `-` and `_` (or a bracketed IPv6 address) and an \
             optional `:port` between 0 and 65535 are accepted"
        )
    })?;
    let seen_by_client = ureq::request("POST", url)
        .request_url()
        .map(|parsed| parsed.host().trim_matches(['[', ']']).to_ascii_lowercase())
        .ok();
    // IP addresses are compared as addresses: the client normalizes them
    // (`[0:0:0:0:0:0:0:1]` is `::1`), and a valid address must not be
    // refused for how it is written.
    let same_host = match (&seen_by_client, host.parse::<IpAddr>()) {
        (Some(seen), Ok(ip)) => seen.parse::<IpAddr>().is_ok_and(|seen_ip| seen_ip == ip),
        (Some(seen), Err(_)) => *seen == host,
        (None, _) => false,
    };
    if !same_host {
        bail!(
            "`base_url` for the {name} provider is ambiguous: the HTTP client does not \
             read the same host in it as ICM does — write it as scheme://host[:port]/path"
        );
    }
    // A fully-qualified name with its trailing dot is the same server.
    let canonical = host.strip_suffix('.').unwrap_or(&host);
    let vendor_host = flavor.is_vendor_host(canonical);
    // The one plain-http case with no legitimate use: a typo'd scheme on
    // the vendor's own host would put the real key on the wire in clear.
    if plain_http && vendor_host {
        bail!(
            "`base_url` for the {name} provider must use https:// — over http:// the API \
             key would reach {canonical} unencrypted"
        );
    }
    Ok(Endpoint {
        url: url.to_string(),
        custom: url != default,
        vendor_host,
        cleartext: plain_http && !is_loopback(canonical),
    })
}

/// Host part of a URL authority: lowercase, without port or IPv6 brackets.
/// `None` for anything but a plain host name, an IPv4 address or a
/// bracketed IPv6 address, optionally followed by a numeric port.
fn host_of(authority: &str) -> Option<String> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']')?;
            let valid = !host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
            (valid.then_some(host)?, after.strip_prefix(':'))
        }
        None => {
            let (host, port) = match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            };
            let valid = !host.is_empty()
                // `_` is not a legal host-name character, but it is what
                // docker-compose service names are made of (`llm_server`),
                // and the HTTP client resolves them.
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
            (valid.then_some(host)?, port)
        }
    };
    if authority.starts_with('[') && port.is_none() && !authority.ends_with(']') {
        return None;
    }
    if let Some(port) = port {
        if port.parse::<u16>().is_err() {
            return None;
        }
    }
    Some(host.to_ascii_lowercase())
}

/// Parsed, not prefix-matched: `127.0.0.1.evil.com` is not loopback.
fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Where a redirect points, and what to do about it, for the redirect
/// error. The target keeps scheme, host and path; the query and fragment
/// are dropped (login redirects carry tokens there). All of it is server
/// text: redacted, then clipped.
fn redirect_target(location: &str, base_url: &str, key: &str) -> (String, &'static str) {
    const SAME_HOST: &str = "the path in `base_url` is probably wrong for this server, or \
                             it is sending you to a login page";
    const OTHER_HOST: &str = "set `base_url` to the final URL (often http:// → https://)";
    let location = location.trim();
    let without_query = location.split(['?', '#']).next().unwrap_or_default();
    if without_query.is_empty() {
        return ("an unnamed location".to_string(), OTHER_HOST);
    }
    let authority_of = |url: &str| {
        url.split_once("://")
            .map(|(_, rest)| rest)
            .or_else(|| url.strip_prefix("//"))
            .map(|rest| {
                let authority = rest.split('/').next().unwrap_or_default();
                authority
                    .rsplit('@')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            })
    };
    match authority_of(without_query) {
        // Relative: same scheme, same host, another path.
        None => (
            format!("{} on the same host", quote(without_query, key, 120)),
            SAME_HOST,
        ),
        Some(target) => {
            let (prefix, rest) = match without_query.split_once("://") {
                Some((scheme, rest)) => (format!("{scheme}://"), rest),
                None => ("//".to_string(), without_query.trim_start_matches('/')),
            };
            let (authority, path) = match rest.find('/') {
                Some(at) => rest.split_at(at),
                None => (rest, ""),
            };
            // Userinfo, if any, is not shown.
            let host = authority.rsplit('@').next().unwrap_or_default();
            let same_host = authority_of(base_url).as_deref() == Some(target.as_str());
            let same_scheme = without_query.starts_with("//")
                || without_query.split_once("://").map(|(scheme, _)| scheme)
                    == base_url.split_once("://").map(|(scheme, _)| scheme);
            (
                quote(&format!("{prefix}{host}{path}"), key, 160),
                if same_host && same_scheme {
                    SAME_HOST
                } else {
                    OTHER_HOST
                },
            )
        }
    }
}

/// Said once per process, on stderr: the detached workers are silent, so
/// `icm config` carries the same warning where someone will read it.
fn warn_cleartext_once(name: &str, base_url: &str) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[icm summarizer] warning: {name} `base_url` ({base_url}) is plain http:// to a \
             non-loopback host — the summarized text, and the API key if one is \
             configured, are sent unencrypted"
        );
    }
}

/// True when an error (or anything in its source chain) is an I/O timeout.
fn is_timeout(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(e) = current {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                return true;
            }
        }
        current = e.source();
    }
    err.to_string().to_ascii_lowercase().contains("timed out")
}

/// Keys shorter than this are placeholders for key-less local servers, not
/// secrets, and are common substrings of ordinary text.
const SHORT_KEY: usize = 8;

/// Remove the key from server text that is about to be quoted.
///
/// A short key is only removed where it stands alone. Blanket replacement of
/// a one-letter placeholder shreds the message it is meant to protect
/// ("conn[redacted]ction r[redacted]fus[redacted]d") and gives the value
/// away in the process.
fn redact(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_string()
    } else if key.len() < SHORT_KEY {
        redact_delimited(text, key)
    } else {
        redact_secret(text, key)
    }
}

/// Remove a real key from any text, ICM's own included — the last line of
/// defense on every error leaving this module. Placeholders (shorter than
/// [`SHORT_KEY`]) are left to [`redact`]: applied to ICM's own wording they
/// would turn "rejected the API key" into "rejected the API [redacted]".
///
/// Besides the key as-is, the spellings a server or gateway may echo it in
/// are removed: JSON-escaped, up to three levels deep (a gateway relaying
/// an upstream body inside a string), with or without `\/`; `\uXXXX`
/// escapes; percent-encoded; HTML entities. Not covered, because no
/// transformation of the text can catch them reliably: the key base64-ed
/// or split across lines.
fn redact_secret(text: &str, key: &str) -> String {
    if key.len() < SHORT_KEY {
        return text.to_string();
    }
    let decoded;
    let text = if text.contains("\\u") {
        decoded = decode_unicode_escapes(text);
        decoded.as_str()
    } else {
        text
    };
    let mut forms = vec![key.to_string(), percent_encode(key), html_escape(key)];
    forms.push(percent_encode(key).to_ascii_lowercase());
    let mut level = key.to_string();
    for _ in 0..3 {
        let quoted = serde_json::to_string(&level).unwrap_or_default();
        level = quoted
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(&quoted)
            .to_string();
        forms.push(level.clone());
        forms.push(level.replace('/', "\\/"));
    }
    forms.push(key.replace('/', "\\/"));
    // Longest first, so a longer spelling is not left half-replaced by a
    // shorter one it contains.
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    forms.dedup();
    let mut out = text.to_string();
    for form in forms.iter().filter(|form| form.len() >= SHORT_KEY) {
        out = out.replace(form.as_str(), "[redacted]");
    }
    out
}

/// `\uXXXX` escapes turned back into characters (BMP only — enough for the
/// ASCII a key is made of). Anything malformed is left as it is.
fn decode_unicode_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\\u") {
        out.push_str(&rest[..at]);
        let hex = rest.get(at + 2..at + 6);
        match hex
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32)
        {
            Some(c) => {
                out.push(c);
                rest = &rest[at + 6..];
            }
            None => {
                out.push_str("\\u");
                rest = &rest[at + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Percent-encoding of everything but RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
}

/// Replace `key` only where it is not part of a longer word.
fn redact_delimited(text: &str, key: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in text.match_indices(key) {
        if at < copied {
            continue;
        }
        let end = at + key.len();
        let glued = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
        if glued(text[..at].chars().next_back()) || glued(text[end..].chars().next()) {
            continue;
        }
        out.push_str(&text[copied..at]);
        out.push_str("[redacted]");
        copied = end;
    }
    out.push_str(&text[copied..]);
    out
}

/// At most `max` chars, cut on a char boundary.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}

/// Server text made safe to put in a message: one line, redacted, clipped —
/// in that order, so a key straddling the cut cannot survive in part.
fn quote(text: &str, key: &str, max: usize) -> String {
    clip(&redact(&printable(text), key), max)
}

/// One line of plain text: runs of blanks collapsed, and every control
/// character (C0, DEL, C1 — ESC, BEL and CSI among them) turned into a
/// blank first. Server text goes to a terminal and into log files; left
/// in, an escape sequence can erase the error line or rewrite the one
/// above it.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The human-readable message of an error object: all three APIs use
/// `{"error": {"message": …}}`, compatible servers sometimes a bare string.
fn error_message(v: &Value) -> Option<String> {
    v.pointer("/error/message")
        .or_else(|| v.get("error").filter(|e| e.is_string()))
        .or_else(|| v.get("message"))
        .or_else(|| v.get("detail"))
        .and_then(Value::as_str)
        .map(str::to_string)
        // An error object with no message we know: show it whole, decoded
        // and re-encoded so the key cannot hide behind `\/`-style escapes.
        .or_else(|| {
            v.get("error")
                .filter(|e| !e.is_null())
                .map(Value::to_string)
        })
}

/// ` (code X)` for an error object that carries one; a number on
/// OpenRouter, a string on OpenAI, server text either way.
fn error_code(error: &Value, key: &str) -> String {
    match &error["code"] {
        Value::Number(n) => format!(" (code {n})"),
        Value::String(s) if !s.is_empty() => format!(" (code {})", quote(s, key, 60)),
        _ => String::new(),
    }
}

/// The part of an error body worth quoting, safe to quote.
fn error_detail(body: &str, key: &str) -> String {
    let message = match serde_json::from_str::<Value>(body) {
        Ok(v) => error_message(&v).unwrap_or_else(|| v.to_string()),
        Err(_) => body.to_string(),
    };
    let quoted = quote(&message, key, 300);
    if quoted.is_empty() {
        return "(no error message in the response)".to_string();
    }
    quoted
}

/// Lines `icm config` prints for one `[*.summarizer]` section. For an
/// API-key provider this says which variable the key is read from and
/// whether it is set — never its value.
pub fn describe_config(provider: &str, model: &str, opts: &ApiOptions) -> Vec<String> {
    let mut lines = vec![format!("provider = {provider}")];
    let kind = ProviderKind::parse(provider).ok();
    let api = kind
        .filter(ProviderKind::is_api_key)
        .map(|k| ApiSummarizer::new(k, opts));
    match (model.is_empty(), &api) {
        (false, _) => lines.push(format!("model = {model}")),
        (true, Some(Ok(s))) if s.vendor_host || s.flavor != Flavor::OpenAi => lines.push(format!(
            "model = {} (provider default)",
            s.flavor.default_model()
        )),
        (true, Some(Ok(_))) => {
            lines.push("model = (not set — required with a custom base_url)".to_string())
        }
        (true, _) => lines.push("model = (provider default)".to_string()),
    }
    match api {
        Some(Ok(s)) => {
            match &s.key_env {
                Some(env) => {
                    let state = if s.load_key().is_ok() {
                        "set"
                    } else {
                        "NOT set"
                    };
                    lines.push(format!(
                        "api_key_env = {env} ({state}; the key itself is never shown)"
                    ));
                }
                None => lines.push(
                    "api_key_env = (none — no key is sent to this base_url; set api_key_env \
                     if the server needs one)"
                        .to_string(),
                ),
            }
            lines.push(format!("base_url = {}", s.base_url));
            if s.cleartext {
                lines.push(
                    "WARNING: base_url is plain http:// to a non-loopback host — the \
                     summarized text, and the API key if one is configured, are sent \
                     unencrypted"
                        .to_string(),
                );
            }
            match (&s.workspace_id, s.flavor) {
                (Some(id), Flavor::Anthropic) => lines.push(format!("workspace_id = {id}")),
                (Some(_), _) => lines.push(
                    "workspace_id = (ignored: only the anthropic provider uses it)".to_string(),
                ),
                (None, _) => {}
            }
        }
        Some(Err(e)) => lines.push(format!("invalid: {e}")),
        None => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Obviously fake, and shaped like a real key (dashes, mixed case) so
    /// the leak checks exercise the realistic case.
    const KEY: &str = "sk-test-NotARealKey-0123456789abcdef";
    const PROMPT: &str = "Task: merge the memory entries below.\n- A\n- B";

    /// Never set: the tests hand the key to `summarize_with_key` directly.
    const TEST_KEY_ENV: &str = "ICM_TEST_PROVIDER_KEY";
    /// Retry pauses short enough for a test run.
    const FAST_BACKOFF: [Duration; 2] = [Duration::from_millis(20), Duration::from_millis(40)];

    const ALL: [ProviderKind; 3] = [
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::Google,
    ];

    /// What the local server saw.
    struct Captured {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Captured {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
        fn json(&self) -> Value {
            serde_json::from_str(&self.body).expect("request body must be JSON")
        }
    }

    /// What the test server captured. Read through a channel with a
    /// deadline rather than by joining the thread: a server that is never
    /// called would block in `accept()` forever and hang the run instead of
    /// failing the test.
    struct Seen(std::sync::mpsc::Receiver<Captured>);

    impl Seen {
        /// The next captured request.
        fn join(&self) -> std::result::Result<Captured, &'static str> {
            self.0
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "the test server was never called")
        }
        /// Every request captured so far.
        fn all(&self) -> Vec<Captured> {
            self.0.try_iter().collect()
        }
    }

    /// One scripted response of the test server.
    struct Canned {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
        delay: Duration,
    }

    fn canned(status: u16, body: impl Into<String>) -> Canned {
        Canned {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    /// HTTP server on a loopback port: answers one request per scripted
    /// response, in order, and reports what it received. No test in this
    /// module talks to anything but this.
    fn serve_seq(responses: Vec<Canned>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let header_end = loop {
                    let n = stream.read(&mut buf).expect("read request");
                    assert!(n > 0, "client closed before sending headers");
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
                let mut lines = head.split("\r\n");
                let mut request_line = lines.next().unwrap_or_default().split(' ');
                let method = request_line.next().unwrap_or_default().to_string();
                let path = request_line.next().unwrap_or_default().to_string();
                let headers: Vec<(String, String)> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    .collect();
                let length: usize = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                while raw.len() < header_end + length {
                    let n = stream.read(&mut buf).expect("read body");
                    assert!(n > 0, "client closed mid-body");
                    raw.extend_from_slice(&buf[..n]);
                }
                // Reported before answering, so the capture is there as soon
                // as the client has its response.
                let _ = tx.send(Captured {
                    method,
                    path,
                    headers,
                    body: String::from_utf8_lossy(&raw[header_end..]).to_string(),
                });

                std::thread::sleep(response.delay);
                let extra: String = response
                    .headers
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}\r\n"))
                    .collect();
                let reply = format!(
                    "HTTP/1.1 {} Test\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n{extra}\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                // The client may already be gone (timeout test).
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, Seen(rx))
    }

    /// One-shot variant of [`serve_seq`].
    fn serve(
        status: u16,
        extra_headers: &'static [(&'static str, &'static str)],
        body: String,
        delay: Duration,
    ) -> (String, Seen) {
        serve_seq(vec![Canned {
            status,
            headers: extra_headers
                .iter()
                .map(|(k, v)| (*k, v.to_string()))
                .collect(),
            body,
            delay,
        }])
    }

    fn ok(body: Value) -> (String, Seen) {
        serve(200, &[], body.to_string(), Duration::ZERO)
    }

    /// A provider pointed at the local server. `base_path` mirrors the path
    /// part of the real default endpoint.
    fn provider(kind: ProviderKind, server: &str) -> ApiSummarizer {
        let base_path = match kind {
            ProviderKind::Anthropic => "",
            ProviderKind::OpenAi => "/v1",
            _ => "/v1beta",
        };
        let mut p = ApiSummarizer::new(
            kind,
            &ApiOptions {
                // Named explicitly: a non-vendor host gets no key otherwise.
                api_key_env: TEST_KEY_ENV.into(),
                base_url: format!("{server}{base_path}"),
                workspace_id: String::new(),
            },
        )
        .expect("valid options");
        p.backoff = FAST_BACKOFF;
        p
    }

    fn request(model: Option<&'static str>) -> SummarizeRequest<'static> {
        SummarizeRequest {
            prompt: PROMPT,
            model,
            max_tokens: 400,
            timeout: Duration::from_secs(5),
        }
    }

    /// A model every flavor accepts even behind a custom `base_url`.
    fn any_model(kind: ProviderKind) -> Option<&'static str> {
        match kind {
            ProviderKind::OpenAi => Some("test-model"),
            _ => None,
        }
    }

    /// A well-formed "here is your summary" 200 body, per wire format.
    fn success_body(kind: ProviderKind, text: &str) -> Value {
        match kind {
            ProviderKind::Anthropic => json!({
                "content": [{ "type": "text", "text": text }],
                "stop_reason": "end_turn",
            }),
            ProviderKind::OpenAi => json!({
                "choices": [{
                    "message": { "role": "assistant", "content": text },
                    "finish_reason": "stop",
                }],
            }),
            _ => json!({
                "candidates": [{
                    "content": { "parts": [{ "text": text }] },
                    "finishReason": "STOP",
                }],
            }),
        }
    }

    fn assert_no_key(context: &str, text: &str) {
        assert!(!text.contains(KEY), "{context}: API key leaked in: {text}");
        // Not even a recognizable fragment of it.
        assert!(
            !text.contains("NotARealKey"),
            "{context}: API key fragment leaked in: {text}"
        );
    }

    // ── request shape and response extraction ────────────────────────────

    #[test]
    fn anthropic_sends_a_messages_request_and_reads_the_text_blocks() {
        let (server, handle) = ok(json!({
            "content": [
                { "type": "thinking", "thinking": "private reasoning", "signature": "x" },
                { "type": "text", "text": "Summary: merged " },
                { "type": "text", "text": "facts\n" },
            ],
            "stop_reason": "end_turn",
        }));
        let out = provider(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap();
        // Thinking blocks dropped, text blocks joined, preamble trimmed.
        assert_eq!(out, "merged facts");

        let seen = handle.join().unwrap();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path, "/v1/messages");
        assert_eq!(seen.header("x-api-key"), Some(KEY));
        assert_eq!(seen.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(seen.header("content-type"), Some("application/json"));
        assert!(seen.header("authorization").is_none());
        let body = seen.json();
        assert_eq!(body["model"], "claude-haiku-4-5", "cheap default model");
        assert_eq!(body["max_tokens"], 2048, "ceiling, not the 400 budget");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], PROMPT);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn openai_sends_a_chat_completions_request_and_reads_the_message() {
        let (server, handle) = ok(success_body(ProviderKind::OpenAi, "  merged facts\n"));
        let out = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("mistral-small-latest")))
            .unwrap();
        assert_eq!(out, "merged facts");

        let seen = handle.join().unwrap();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path, "/v1/chat/completions");
        let bearer = format!("Bearer {KEY}");
        assert_eq!(seen.header("authorization"), Some(bearer.as_str()));
        assert!(seen.header("x-api-key").is_none());
        let body = seen.json();
        assert_eq!(body["model"], "mistral-small-latest");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], PROMPT);
        // A compatible server gets the field they all implement.
        assert_eq!(body["max_tokens"], 2048);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(
            body.get("reasoning_effort").is_none(),
            "a compatible server may reject the field"
        );
    }

    /// Against OpenAI's own endpoint the cap must be `max_completion_tokens`
    /// (reasoning models reject `max_tokens`) and the default model applies.
    #[test]
    fn openai_default_endpoint_uses_default_model_and_max_completion_tokens() {
        let (server, handle) = ok(success_body(ProviderKind::OpenAi, "ok"));
        let mut p = ApiSummarizer::new(ProviderKind::OpenAi, &ApiOptions::default()).unwrap();
        assert_eq!(p.base_url, "https://api.openai.com/v1");
        assert!(!p.custom_base && p.vendor_host);
        // Same object, re-pointed at the local server: no real endpoint is
        // ever contacted.
        p.base_url = format!("{server}/v1");
        assert_eq!(p.summarize_with_key(KEY, &request(None)).unwrap(), "ok");

        let body = handle.join().unwrap().json();
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["max_completion_tokens"], 8192);
        assert!(body.get("max_tokens").is_none());
        // The default model reasons at `medium` unless told not to.
        assert_eq!(body["reasoning_effort"], "none");
    }

    #[test]
    fn openai_reads_typed_content_parts() {
        let (server, _handle) = ok(json!({
            "choices": [{
                "message": { "content": [
                    { "type": "text", "text": "part one, " },
                    { "type": "text", "text": "part two" },
                ] },
                "finish_reason": "stop",
            }],
        }));
        let out = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("m")))
            .unwrap();
        assert_eq!(out, "part one, part two");
    }

    #[test]
    fn google_sends_a_generate_content_request_and_reads_the_parts() {
        let (server, handle) = ok(json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "text": "thinking out loud", "thought": true },
                    { "text": "merged " },
                    { "text": "facts" },
                ] },
                "finishReason": "STOP",
            }],
        }));
        let out = provider(ProviderKind::Google, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap();
        assert_eq!(out, "merged facts", "thought parts are not the answer");

        let seen = handle.join().unwrap();
        assert_eq!(seen.method, "POST");
        assert_eq!(
            seen.path, "/v1beta/models/gemini-3.5-flash-lite:generateContent",
            "default model in the path, and the key nowhere in the URL"
        );
        assert_eq!(seen.header("x-goog-api-key"), Some(KEY));
        assert!(seen.header("authorization").is_none());
        let body = seen.json();
        assert_eq!(body["contents"][0]["parts"][0]["text"], PROMPT);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 2048);
    }

    #[test]
    fn configured_model_overrides_the_default_on_every_provider() {
        for (kind, model, expect_in_body) in [
            (ProviderKind::Anthropic, "claude-sonnet-5-5", true),
            (ProviderKind::OpenAi, "gpt-custom", true),
            // The `models/` prefix from the Gemini docs is accepted.
            (ProviderKind::Google, "models/gemini-3.8-flash", false),
        ] {
            let (server, handle) = ok(success_body(kind, "ok"));
            provider(kind, &server)
                .summarize_with_key(KEY, &request(Some(model)))
                .unwrap();
            let seen = handle.join().unwrap();
            if expect_in_body {
                assert_eq!(seen.json()["model"], model, "{kind:?}");
            } else {
                assert_eq!(seen.path, "/v1beta/models/gemini-3.8-flash:generateContent");
            }
        }
    }

    #[test]
    fn output_ceiling_leaves_headroom_over_the_budget() {
        for kind in ALL {
            // On the vendor's own API the floor holds up to a 2048 budget,
            // then 4x takes over.
            let vendor = build(kind, &ApiOptions::default());
            assert_eq!(vendor.output_ceiling(400), 8192);
            assert_eq!(vendor.output_ceiling(2048), 8192);
            assert_eq!(vendor.output_ceiling(2049), 8196);
            assert_eq!(vendor.output_ceiling(3000), 12_000);
            // Anywhere else the context window is unknown: a floor of 8192
            // alone overflows an 8K model on every request.
            let compat = provider(kind, "http://127.0.0.1:9");
            assert_eq!(compat.output_ceiling(400), 2048);
            assert_eq!(compat.output_ceiling(512), 2048);
            assert_eq!(compat.output_ceiling(513), 2052);
        }
        // Anthropic answers 400 above the smallest model's output limit.
        let anthropic = build(ProviderKind::Anthropic, &ApiOptions::default());
        assert_eq!(anthropic.output_ceiling(16_000), 64_000);
        assert_eq!(anthropic.output_ceiling(20_000), 64_000);
        assert_eq!(anthropic.output_ceiling(usize::MAX), 64_000);
        // No documented bound to enforce for the other two.
        let openai = build(ProviderKind::OpenAi, &ApiOptions::default());
        assert_eq!(openai.output_ceiling(20_000), 80_000);
        let google = build(ProviderKind::Google, &ApiOptions::default());
        assert_eq!(google.output_ceiling(usize::MAX), usize::MAX);
    }

    // ── error cases ──────────────────────────────────────────────────────

    /// An error body that parrots the key back, the worst case for leaks.
    fn hostile_error_body() -> String {
        json!({ "error": { "type": "x", "message": format!("problem with key {KEY} here") } })
            .to_string()
    }

    #[test]
    fn missing_key_names_the_variable_to_set() {
        for kind in ALL {
            // Default variable name in the message, without depending on the
            // host environment: check the name the provider resolved.
            let p = ApiSummarizer::new(kind, &ApiOptions::default()).unwrap();
            let expected = match kind {
                ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
                ProviderKind::OpenAi => "OPENAI_API_KEY",
                _ => "GEMINI_API_KEY",
            };
            assert_eq!(p.key_env.as_deref(), Some(expected));

            // A variable guaranteed unset: the request must never be sent.
            let p = ApiSummarizer::new(
                kind,
                &ApiOptions {
                    api_key_env: "ICM_TEST_KEY_THAT_IS_NEVER_SET".into(),
                    base_url: "http://127.0.0.1:9".into(),
                    workspace_id: String::new(),
                },
            )
            .unwrap();
            let err = p.summarize(&request(Some("m"))).unwrap_err().to_string();
            assert!(err.contains("no API key"), "{err}");
            assert!(
                err.contains("export ICM_TEST_KEY_THAT_IS_NEVER_SET="),
                "must say which variable to set: {err}"
            );
        }
    }

    #[test]
    fn key_is_read_from_the_configured_variable() {
        // A name unique to this test, so no other test races on it.
        let var = "ICM_TEST_SUMMARIZER_KEY_READ";
        unsafe { std::env::set_var(var, format!("  {KEY}\n")) };
        let (server, handle) = ok(success_body(ProviderKind::Anthropic, "ok"));
        let p = ApiSummarizer::new(
            ProviderKind::Anthropic,
            &ApiOptions {
                api_key_env: var.into(),
                base_url: server,
                workspace_id: String::new(),
            },
        )
        .unwrap();
        let out = p.summarize(&request(None));
        unsafe { std::env::remove_var(var) };
        assert_eq!(out.unwrap(), "ok");
        // Surrounding whitespace from a sloppy `export` is trimmed.
        assert_eq!(handle.join().unwrap().header("x-api-key"), Some(KEY));
    }

    #[test]
    fn unauthorized_points_at_the_key_variable_and_quotes_no_server_text() {
        for kind in ALL {
            let (server, _h) = serve(401, &[], hostile_error_body(), Duration::ZERO);
            let p = provider(kind, &server);
            let err = p
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("HTTP 401"), "{kind:?}: {err}");
            assert!(err.contains("rejected the API key"), "{kind:?}: {err}");
            assert!(err.contains(TEST_KEY_ENV), "{kind:?}: {err}");
            assert!(!err.contains("problem with key"), "401 body quoted: {err}");
            assert_no_key("401", &err);
        }
    }

    #[test]
    fn forbidden_explains_the_permission_problem() {
        for kind in ALL {
            let (server, _h) = serve(403, &[], hostile_error_body(), Duration::ZERO);
            let p = provider(kind, &server);
            let err = p
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("HTTP 403"), "{kind:?}: {err}");
            assert!(err.contains("not allowed to use model"), "{kind:?}: {err}");
            assert!(err.contains(TEST_KEY_ENV), "{kind:?}: {err}");
            assert!(err.contains("[redacted]"), "{kind:?}: {err}");
            assert_no_key("403", &err);
        }
    }

    #[test]
    fn rate_limit_reports_retry_after_and_the_provider_message() {
        for kind in ALL {
            let (server, _h) = serve(
                429,
                &[("retry-after", "17")],
                hostile_error_body(),
                Duration::ZERO,
            );
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("HTTP 429"), "{kind:?}: {err}");
            assert!(err.contains("rate limit or quota"), "{kind:?}: {err}");
            assert!(err.contains("retry-after=17"), "{kind:?}: {err}");
            assert!(err.contains("retry later"), "{kind:?}: {err}");
            assert_no_key("429", &err);
        }
    }

    #[test]
    fn timeout_cuts_the_wait_short_and_names_the_setting_to_raise() {
        for kind in ALL {
            let (server, _detached) = serve(
                200,
                &[],
                success_body(kind, "too late").to_string(),
                Duration::from_millis(1500),
            );
            let req = SummarizeRequest {
                timeout: Duration::from_millis(250),
                ..request(any_model(kind))
            };
            let started = std::time::Instant::now();
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &req)
                .unwrap_err()
                .to_string();
            assert!(
                started.elapsed() < Duration::from_millis(1400),
                "{kind:?}: the timeout must cut the wait short"
            );
            assert!(err.contains("timed out after 250ms"), "{kind:?}: {err}");
            assert!(err.contains("timeout_secs"), "{kind:?}: {err}");
            assert_no_key("timeout", &err);
        }
    }

    /// Empty output is an error (so callers take their provider-failed
    /// fallback instead of treating "" as a summary), whatever shape the
    /// emptiness takes.
    #[test]
    fn empty_response_is_an_actionable_error() {
        let cases: Vec<(ProviderKind, Value)> = vec![
            (
                ProviderKind::Anthropic,
                json!({ "content": [], "stop_reason": "end_turn" }),
            ),
            (
                ProviderKind::Anthropic,
                success_body(ProviderKind::Anthropic, "  \n"),
            ),
            (
                ProviderKind::OpenAi,
                json!({ "choices": [{ "message": { "content": null }, "finish_reason": "stop" }] }),
            ),
            (ProviderKind::OpenAi, success_body(ProviderKind::OpenAi, "")),
            (
                ProviderKind::Google,
                json!({ "candidates": [{ "content": { "parts": [] }, "finishReason": "STOP" }] }),
            ),
            (
                ProviderKind::Google,
                json!({ "candidates": [{ "finishReason": "STOP" }] }),
            ),
        ];
        for (kind, body) in cases {
            let (server, _h) = ok(body.clone());
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("empty response"), "{kind:?} {body}: {err}");
            assert!(err.contains("model="), "must name the model: {err}");
            assert_no_key("empty", &err);
        }
    }

    /// A reasoning model that burns the whole cap, or an answer cut
    /// mid-sentence: both must fail loudly rather than return a fragment.
    #[test]
    fn truncated_response_is_an_error_even_with_partial_text() {
        let cases: Vec<(ProviderKind, Value)> = vec![
            (
                ProviderKind::Anthropic,
                json!({ "content": [{ "type": "thinking", "thinking": "" }], "stop_reason": "max_tokens" }),
            ),
            (
                ProviderKind::Anthropic,
                json!({ "content": [{ "type": "text", "text": "half a sum" }], "stop_reason": "max_tokens" }),
            ),
            (
                ProviderKind::OpenAi,
                json!({ "choices": [{ "message": { "content": "" }, "finish_reason": "length" }] }),
            ),
            (
                ProviderKind::Google,
                json!({ "candidates": [{ "content": { "parts": [{ "text": "half" }] }, "finishReason": "MAX_TOKENS" }] }),
            ),
        ];
        for (kind, body) in cases {
            let (server, _h) = ok(body);
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("2048-token output ceiling"), "{kind:?}: {err}");
            // The advice names the budget that actually moves the ceiling:
            // anything up to 512 still yields 2048 on a non-vendor host.
            assert!(
                err.contains("`max_tokens = 400` gives a ceiling of 2048"),
                "{kind:?}: {err}"
            );
            assert!(err.contains("above 512"), "{kind:?}: {err}");
        }
    }

    #[test]
    fn refusals_and_safety_blocks_are_errors() {
        let cases: Vec<(ProviderKind, Value, &str)> = vec![
            (
                ProviderKind::Anthropic,
                json!({ "content": [], "stop_reason": "refusal", "stop_details": { "category": "cyber" } }),
                "category=cyber",
            ),
            (
                ProviderKind::OpenAi,
                json!({ "choices": [{ "message": { "content": null, "refusal": "I can't help" }, "finish_reason": "stop" }] }),
                "refusal: I can't help",
            ),
            (
                ProviderKind::Google,
                json!({ "promptFeedback": { "blockReason": "SAFETY" } }),
                "blockReason=SAFETY",
            ),
            (
                ProviderKind::Google,
                json!({ "candidates": [{ "finishReason": "SAFETY" }] }),
                "finishReason=SAFETY",
            ),
        ];
        for (kind, body, expected) in cases {
            let (server, _h) = ok(body);
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("declined to answer"), "{kind:?}: {err}");
            assert!(err.contains(expected), "{kind:?}: {err}");
        }
    }

    #[test]
    fn other_http_errors_quote_the_redacted_provider_message() {
        for (status, phrase) in [
            (404, "check the model name"),
            (400, "rejected the request"),
            (529, "retry later"),
        ] {
            for kind in ALL {
                // 529 is retried twice, so it is scripted three times.
                let (server, _h) = serve_seq(
                    (0..3)
                        .map(|_| canned(status, hostile_error_body()))
                        .collect(),
                );
                let err = provider(kind, &server)
                    .summarize_with_key(KEY, &request(Some("explicit-model")))
                    .unwrap_err()
                    .to_string();
                assert!(err.contains(&format!("HTTP {status}")), "{kind:?}: {err}");
                assert!(err.contains(phrase), "{kind:?}: {err}");
                assert!(err.contains("problem with key [redacted] here"), "{err}");
                assert_no_key("http error", &err);
            }
        }
    }

    #[test]
    fn unexpected_bodies_are_errors_not_panics() {
        for kind in ALL {
            for body in [
                "<html>gateway</html>",
                "{}",
                "[]",
                "{\"content\": 3, \"choices\": []}",
            ] {
                let (server, _h) = serve(200, &[], body.to_string(), Duration::ZERO);
                let err = provider(kind, &server)
                    .summarize_with_key(KEY, &request(any_model(kind)))
                    .unwrap_err()
                    .to_string();
                assert!(
                    err.contains("not JSON") || err.contains("unexpected response"),
                    "{kind:?} {body}: {err}"
                );
                assert!(err.contains("check `base_url`"), "{kind:?} {body}: {err}");
            }
        }
    }

    #[test]
    fn unreachable_endpoint_is_reported_without_the_key() {
        // Port 1 (tcpmux) has no listener on any dev or CI machine. A
        // bind-then-drop ephemeral port would race the other tests' servers,
        // which grab ephemeral ports concurrently.
        for kind in ALL {
            let err = provider(kind, "http://127.0.0.1:1")
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            // A closed port is refused at once on Unix. Windows retries the
            // connection instead, and it ends as a host that does not answer.
            let refused = err.contains("request failed") && err.contains("check network access");
            let unanswered =
                err.contains("could not connect to") && err.contains("the host is not answering");
            assert!(refused || unanswered, "{kind:?}: {err}");
            assert_no_key("transport", &err);
        }
    }

    // ── key hygiene and option validation ────────────────────────────────

    /// The HTTP client quotes the full header line when a value is not
    /// sendable; that must be caught before it gets there.
    #[test]
    fn unsendable_key_is_rejected_without_being_printed() {
        let var = "ICM_TEST_SUMMARIZER_KEY_UNSENDABLE";
        unsafe { std::env::set_var(var, format!("{KEY} trailing words")) };
        let p = ApiSummarizer::new(
            ProviderKind::OpenAi,
            &ApiOptions {
                api_key_env: var.into(),
                base_url: "http://127.0.0.1:9".into(),
                workspace_id: String::new(),
            },
        )
        .unwrap();
        let err = p.summarize(&request(Some("m"))).unwrap_err().to_string();
        unsafe { std::env::remove_var(var) };
        assert!(err.contains(var), "{err}");
        assert!(err.contains("re-export"), "{err}");
        assert_no_key("unsendable key", &err);
    }

    /// The classic mistake: the key pasted where the variable name goes.
    #[test]
    fn a_key_pasted_into_api_key_env_is_rejected_and_not_echoed() {
        for kind in ALL {
            for pasted in [
                KEY,
                "AIzaSyNotARealKey0123456789abcdefghijk",
                "sk-ant-api03-NotARealKey",
            ] {
                let err = ApiSummarizer::new(
                    kind,
                    &ApiOptions {
                        api_key_env: pasted.into(),
                        base_url: String::new(),
                        workspace_id: String::new(),
                    },
                )
                .err()
                .expect("a key is not a variable name")
                .to_string();
                assert!(err.contains("NAME of an environment variable"), "{err}");
                assert!(!err.contains(pasted), "pasted key echoed: {err}");
                assert_no_key("api_key_env", &err);

                let lines = describe_config(
                    kind.as_str(),
                    "",
                    &ApiOptions {
                        api_key_env: pasted.into(),
                        base_url: String::new(),
                        workspace_id: String::new(),
                    },
                )
                .join("\n");
                assert!(lines.contains("invalid:"), "{lines}");
                assert!(
                    !lines.contains(pasted),
                    "pasted key shown by config: {lines}"
                );
            }
        }
        // Ordinary names are fine.
        for name in ["OPENROUTER_API_KEY", "MY_KEY_2", "_PRIVATE"] {
            assert_eq!(
                resolve_key_env(Flavor::OpenAi, name, true)
                    .unwrap()
                    .as_deref(),
                Some(name)
            );
        }
    }

    #[test]
    fn base_url_is_validated_and_never_carries_credentials() {
        let e = resolve_base_url(Flavor::OpenAi, "https://openrouter.ai/api/v1/").unwrap();
        assert_eq!(e.url, "https://openrouter.ai/api/v1");
        assert!(e.custom && !e.vendor_host && !e.cleartext);
        let e = resolve_base_url(Flavor::OpenAi, "  ").unwrap();
        assert_eq!(e.url, "https://api.openai.com/v1");
        assert!(!e.custom && e.vendor_host);
        // Spelling out the default is not "custom".
        assert!(
            !resolve_base_url(Flavor::Anthropic, "https://api.anthropic.com/")
                .unwrap()
                .custom
        );

        assert!(resolve_base_url(Flavor::OpenAi, "openrouter.ai/api/v1").is_err());
        assert!(resolve_base_url(Flavor::OpenAi, "https://").is_err());
        let err = resolve_base_url(Flavor::OpenAi, "https://user:hunter2secret@host/v1")
            .err()
            .expect("credentials in the URL are refused")
            .to_string();
        assert!(err.contains("must not embed credentials"), "{err}");
        assert!(!err.contains("hunter2secret"), "{err}");
    }

    #[test]
    fn openai_compatible_server_requires_an_explicit_model() {
        let p = ApiSummarizer::new(
            ProviderKind::OpenAi,
            &ApiOptions {
                api_key_env: String::new(),
                base_url: "http://127.0.0.1:9/v1".into(),
                workspace_id: String::new(),
            },
        )
        .unwrap();
        let err = p
            .summarize_with_key(KEY, &request(None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs an explicit model"), "{err}");
        assert!(err.contains("model = "), "{err}");
        // Whitespace-only counts as unset.
        assert!(p.summarize_with_key(KEY, &request(Some("  "))).is_err());
    }

    #[test]
    fn google_model_cannot_rewrite_the_url() {
        let p = ApiSummarizer::new(ProviderKind::Google, &ApiOptions::default()).unwrap();
        for bad in ["x/../../other", "m?key=1", "a b", "models/"] {
            assert!(p.resolve_model(Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(
            p.resolve_model(Some("gemini-3.8-flash")).unwrap(),
            ("gemini-3.8-flash".to_string(), false)
        );
        assert_eq!(
            p.resolve_model(None).unwrap(),
            ("gemini-3.5-flash-lite".to_string(), true)
        );
    }

    #[test]
    fn describe_config_reports_key_presence_but_never_the_key() {
        let var = "ICM_TEST_SUMMARIZER_KEY_DESCRIBE";
        let opts = ApiOptions {
            api_key_env: var.into(),
            base_url: String::new(),
            workspace_id: String::new(),
        };
        unsafe { std::env::remove_var(var) };
        let unset = describe_config("anthropic", "", &opts).join("\n");
        unsafe { std::env::set_var(var, KEY) };
        let set = describe_config("anthropic", "", &opts).join("\n");
        unsafe { std::env::remove_var(var) };

        assert!(
            unset.contains(&format!("api_key_env = {var} (NOT set")),
            "{unset}"
        );
        assert!(set.contains(&format!("api_key_env = {var} (set")), "{set}");
        assert!(set.contains("provider = anthropic"), "{set}");
        assert!(
            set.contains("model = claude-haiku-4-5 (provider default)"),
            "{set}"
        );
        assert!(
            set.contains("base_url = https://api.anthropic.com"),
            "{set}"
        );
        assert_no_key("icm config", &set);

        // CLI providers have no key line at all.
        let cli = describe_config("claude", "", &ApiOptions::default()).join("\n");
        assert_eq!(cli, "provider = claude\nmodel = (provider default)");
        let none = describe_config("none", "m", &ApiOptions::default()).join("\n");
        assert_eq!(none, "provider = none\nmodel = m");
    }

    #[test]
    fn error_detail_redacts_before_clipping() {
        // The key sits right on the 300-char cut: clipping first would leave
        // its head in the message.
        let body = format!("{}{KEY}", "x".repeat(290));
        let detail = error_detail(&body, KEY);
        assert_no_key("clip", &detail);
        assert!(detail.contains("[redacted]"), "{detail}");
        assert_eq!(error_detail("", KEY), "(no error message in the response)");
        assert_eq!(
            error_detail("{\"error\":\"plain string\"}", KEY),
            "plain string"
        );
        assert_eq!(error_detail("{\"detail\":\"Not Found\"}", KEY), "Not Found");
    }
    // ── retries, endpoints, keys and output caps ─────────────────────────

    /// Options pointing `kind` at the local server, to adjust before
    /// building the provider.
    fn options(kind: ProviderKind, server: &str) -> ApiOptions {
        let base_path = match kind {
            ProviderKind::Anthropic => "",
            ProviderKind::OpenAi => "/v1",
            _ => "/v1beta",
        };
        ApiOptions {
            api_key_env: TEST_KEY_ENV.into(),
            base_url: format!("{server}{base_path}"),
            workspace_id: String::new(),
        }
    }

    fn build(kind: ProviderKind, opts: &ApiOptions) -> ApiSummarizer {
        let mut p = ApiSummarizer::new(kind, opts).expect("valid options");
        p.backoff = FAST_BACKOFF;
        p
    }

    /// The provider on its vendor defaults (default model, key variable,
    /// request fields), re-pointed at the local server so that nothing real
    /// is ever contacted.
    fn vendor_default(kind: ProviderKind, server: &str) -> ApiSummarizer {
        let mut p = build(kind, &ApiOptions::default());
        assert!(p.vendor_host && !p.custom_base);
        p.base_url = options(kind, server).base_url;
        p
    }

    fn error_body(message: &str) -> String {
        json!({ "error": { "type": "x", "message": message } }).to_string()
    }

    #[test]
    fn anthropic_sends_the_workspace_header_only_when_configured() {
        let (server, seen) = ok(success_body(ProviderKind::Anthropic, "ok"));
        let mut opts = options(ProviderKind::Anthropic, &server);
        opts.workspace_id = "wrkspc_01AbCdEf".into();
        build(ProviderKind::Anthropic, &opts)
            .summarize_with_key(KEY, &request(None))
            .unwrap();
        let got = seen.join().unwrap();
        assert_eq!(
            got.header("anthropic-workspace-id"),
            Some("wrkspc_01AbCdEf")
        );
        assert_eq!(got.header("x-api-key"), Some(KEY));

        // A key scoped to one workspace needs no header: none is sent.
        let (server, seen) = ok(success_body(ProviderKind::Anthropic, "ok"));
        provider(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap();
        assert!(
            seen.join()
                .unwrap()
                .header("anthropic-workspace-id")
                .is_none()
        );

        // It is an Anthropic header: the other providers never send it.
        for kind in [ProviderKind::OpenAi, ProviderKind::Google] {
            let (server, seen) = ok(success_body(kind, "ok"));
            let mut opts = options(kind, &server);
            opts.workspace_id = "wrkspc_01AbCdEf".into();
            build(kind, &opts)
                .summarize_with_key(KEY, &request(Some("m")))
                .unwrap();
            assert!(
                seen.join()
                    .unwrap()
                    .header("anthropic-workspace-id")
                    .is_none()
            );
            let lines = describe_config(kind.as_str(), "m", &opts).join("\n");
            assert!(lines.contains("workspace_id = (ignored"), "{lines}");
        }

        // It goes into a header: nothing that could break one is accepted.
        for bad in ["wrkspc 01", "wrkspc_01\r\nx-evil: 1", "wrkspc/01"] {
            let opts = ApiOptions {
                workspace_id: bad.into(),
                ..ApiOptions::default()
            };
            assert!(
                ApiSummarizer::new(ProviderKind::Anthropic, &opts).is_err(),
                "{bad:?}"
            );
        }
        let shown = describe_config(
            "anthropic",
            "",
            &ApiOptions {
                workspace_id: "wrkspc_01AbCdEf".into(),
                ..ApiOptions::default()
            },
        )
        .join("\n");
        assert!(shown.contains("workspace_id = wrkspc_01AbCdEf"), "{shown}");
    }

    #[test]
    fn anthropic_400_about_the_workspace_header_says_what_to_set() {
        let (server, _seen) = serve(
            400,
            &[],
            error_body(
                "anthropic-workspace-id is required when authenticating with an \
                 identity-linked API key; send the id of the workspace this request acts in.",
            ),
            Duration::ZERO,
        );
        let err = provider(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("workspace_id = "), "{err}");
        assert!(err.contains("scoped to one workspace"), "{err}");
    }

    #[test]
    fn transient_failures_are_retried_within_the_timeout() {
        for kind in ALL {
            // One overloaded answer, then the service is back.
            let (server, seen) = serve_seq(vec![
                canned(529, error_body("Overloaded")),
                canned(200, success_body(kind, "ok").to_string()),
            ]);
            let out = provider(kind, &server).summarize_with_key(KEY, &request(any_model(kind)));
            assert_eq!(out.unwrap(), "ok", "{kind:?}");
            assert_eq!(seen.all().len(), 2, "{kind:?}");

            // A rate limit with no `retry-after` and no sign of a spent
            // quota clears too.
            let (server, seen) = serve_seq(vec![
                canned(429, error_body("slow down")),
                canned(200, success_body(kind, "ok").to_string()),
            ]);
            let out = provider(kind, &server).summarize_with_key(KEY, &request(any_model(kind)));
            assert_eq!(out.unwrap(), "ok", "{kind:?}");
            assert_eq!(seen.all().len(), 2, "{kind:?}");

            // Still down after two retries: the provider's error, after
            // three requests and no more.
            let (server, seen) = serve_seq(
                (0..5)
                    .map(|_| canned(529, error_body("Overloaded")))
                    .collect(),
            );
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("HTTP 529"), "{kind:?}: {err}");
            assert_eq!(seen.all().len(), 3, "{kind:?}");
        }
    }

    #[test]
    fn retry_after_is_honored_before_the_next_attempt() {
        let kind = ProviderKind::Anthropic;
        let mut limited = canned(429, error_body("rate limited"));
        limited.headers.push(("retry-after", "1".into()));
        let (server, seen) = serve_seq(vec![
            limited,
            canned(200, success_body(kind, "ok").to_string()),
        ]);
        let started = Instant::now();
        let out = provider(kind, &server).summarize_with_key(KEY, &request(None));
        assert_eq!(out.unwrap(), "ok");
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "waited only {:?}",
            started.elapsed()
        );
        assert_eq!(seen.all().len(), 2);
    }

    /// Each case is followed by a 200 the provider must never reach.
    #[test]
    fn failures_that_waiting_cannot_fix_are_not_retried() {
        let spent = json!({ "error": { "type": "rate_limit_error", "message": "cap",
            "details": { "error_code": "enforced_spend_limit_reached" } } })
        .to_string();
        let quota = json!({ "error": { "message": "quota", "type": "insufficient_quota",
            "code": "insufficient_quota" } })
        .to_string();
        let cases: Vec<(&str, Canned)> = vec![
            ("retry-after beyond the timeout", {
                let mut c = canned(429, error_body("rate limited"));
                c.headers.push(("retry-after", "17".into()));
                c
            }),
            ("anthropic spend cap", canned(429, spent)),
            ("openai exhausted quota", canned(429, quota)),
            ("bad key", canned(401, error_body("no"))),
            ("bad request", canned(400, error_body("no"))),
            ("not found", canned(404, error_body("no"))),
        ];
        for (what, first) in cases {
            for kind in ALL {
                let first = Canned {
                    status: first.status,
                    headers: first.headers.clone(),
                    body: first.body.clone(),
                    delay: first.delay,
                };
                let (server, seen) = serve_seq(vec![
                    first,
                    canned(200, success_body(kind, "late").to_string()),
                ]);
                let started = Instant::now();
                let out = provider(kind, &server).summarize_with_key(KEY, &request(Some("m")));
                assert!(out.is_err(), "{what} / {kind:?}: {out:?}");
                assert_eq!(seen.all().len(), 1, "{what} / {kind:?}: retried");
                assert!(started.elapsed() < Duration::from_secs(2), "{what}: slept");
            }
        }
    }

    #[test]
    fn openai_regional_endpoints_are_openai_not_third_party() {
        for url in [
            "https://eu.api.openai.com/v1",
            "https://us.api.openai.com/v1/",
            "https://EU.api.openai.com:443/v1",
            "https://API.OPENAI.COM/v1",
        ] {
            let opts = ApiOptions {
                base_url: url.into(),
                ..ApiOptions::default()
            };
            let p = build(ProviderKind::OpenAi, &opts);
            assert!(p.vendor_host, "{url}");
            assert_eq!(p.key_env.as_deref(), Some("OPENAI_API_KEY"), "{url}");
            // The default model exists there, and gets OpenAI's fields.
            let (model, is_default) = p.resolve_model(None).expect(url);
            assert_eq!((model.as_str(), is_default), ("gpt-6-luna", true));
            let (_, body) = p.build_request(&model, PROMPT, 8192, p.openai_cap_field());
            assert_eq!(body["max_completion_tokens"], 8192, "{url}");
            assert!(body.get("max_tokens").is_none(), "{url}");
            assert_eq!(body["reasoning_effort"], "none", "{url}");
            let shown = describe_config("openai", "", &opts).join("\n");
            assert!(
                shown.contains("model = gpt-6-luna (provider default)"),
                "{shown}"
            );
        }
        for url in [
            "https://evilapi.openai.com/v1",
            "https://api.openai.com.evil.example/v1",
            "https://openrouter.ai/api/v1",
        ] {
            let opts = ApiOptions {
                base_url: url.into(),
                ..ApiOptions::default()
            };
            let p = build(ProviderKind::OpenAi, &opts);
            assert!(!p.vendor_host, "{url}");
            assert_eq!(p.key_env, None, "{url}: OPENAI_API_KEY must not go there");
            assert!(p.resolve_model(None).is_err(), "{url}");
            assert_eq!(p.openai_cap_field(), "max_tokens", "{url}");
        }
    }

    /// `reasoning_effort: none` is right for the default model only: other
    /// OpenAI models answer 400 to it.
    #[test]
    fn openai_reasoning_effort_is_only_sent_for_the_default_model() {
        let p = build(ProviderKind::OpenAi, &ApiOptions::default());
        let (_, body) = p.build_request("gpt-6-luna", PROMPT, 8192, "max_completion_tokens");
        assert_eq!(body["reasoning_effort"], "none");
        let (_, body) = p.build_request("gpt-6-astra", PROMPT, 8192, "max_completion_tokens");
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn openai_gateway_that_rejects_the_cap_field_gets_the_other_one() {
        let rejection = json!({ "error": {
            "message": "Unsupported parameter: 'max_tokens' is not supported with this model. \
                        Use 'max_completion_tokens' instead.",
            "type": "invalid_request_error",
            "param": "max_tokens",
            "code": "unsupported_parameter",
        } })
        .to_string();
        let (server, seen) = serve_seq(vec![
            canned(400, rejection.clone()),
            canned(200, success_body(ProviderKind::OpenAi, "ok").to_string()),
        ]);
        let out = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("o4-mini")))
            .unwrap();
        assert_eq!(out, "ok");
        let sent = seen.all();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].json()["max_tokens"], 2048);
        assert_eq!(sent[1].json()["max_completion_tokens"], 2048);
        assert!(sent[1].json().get("max_tokens").is_none());

        // Only once: a server that rejects both gets its error reported.
        let (server, seen) = serve_seq(vec![
            canned(400, rejection.clone()),
            canned(400, rejection),
            canned(200, success_body(ProviderKind::OpenAi, "never").to_string()),
        ]);
        let err = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("o4-mini")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 400"), "{err}");
        assert_eq!(seen.all().len(), 2);
    }

    #[test]
    fn anthropic_output_cap_never_exceeds_what_every_model_accepts() {
        let truncated = json!({
            "content": [{ "type": "text", "text": "half" }],
            "stop_reason": "max_tokens",
        });
        let (server, seen) = ok(truncated);
        let req = SummarizeRequest {
            max_tokens: 20_000,
            ..request(None)
        };
        let err = provider(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &req)
            .unwrap_err()
            .to_string();
        assert_eq!(seen.join().unwrap().json()["max_tokens"], 64_000);
        assert!(err.contains("64000-token output ceiling"), "{err}");
        // At the bound, "raise max_tokens" would be a dead end.
        assert!(err.contains("largest cap this provider accepts"), "{err}");
        assert!(!err.contains("to raise it"), "{err}");

        // A server that rejects the cap: say where the number comes from —
        // the user configured a quarter of it.
        let (server, _seen) = serve(
            400,
            &[],
            error_body("max_tokens: 80000 > 64000, which is the maximum allowed"),
            Duration::ZERO,
        );
        let err = provider(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &req)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("the value sent (64000) is ICM's output ceiling"),
            "{err}"
        );
        assert!(err.contains("lower `max_tokens`"), "{err}");
    }

    #[test]
    fn not_found_on_the_built_in_default_model_says_so() {
        for kind in ALL {
            let (server, _seen) = serve(404, &[], error_body("model: gone"), Duration::ZERO);
            let err = vendor_default(kind, &server)
                .summarize_with_key(KEY, &request(None))
                .unwrap_err()
                .to_string();
            assert!(err.contains("ICM's built-in default"), "{kind:?}: {err}");
            assert!(err.contains("model = "), "{kind:?}: {err}");
            assert!(!err.contains("check the model name"), "{kind:?}: {err}");

            // A model the user wrote, even the same one: their name to check.
            let (server, _seen) = serve(404, &[], error_body("model: gone"), Duration::ZERO);
            let p = vendor_default(kind, &server);
            let named = p.flavor.default_model();
            let err = p
                .summarize_with_key(KEY, &request(Some(named)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("check the model name"), "{kind:?}: {err}");
            assert!(err.contains("has access to it"), "{kind:?}: {err}");
        }
    }

    /// The vendor's key variable belongs to the vendor's host. Pointing
    /// `base_url` at a local or third-party server must not ship it there.
    #[test]
    fn a_third_party_host_gets_no_key_unless_api_key_env_names_one() {
        for kind in ALL {
            let (server, seen) = ok(success_body(kind, "ok"));
            let mut opts = options(kind, &server);
            opts.api_key_env = String::new();
            let p = build(kind, &opts);
            assert_eq!(p.key_env, None, "{kind:?}");
            // `summarize`, not `summarize_with_key`: the whole path, with
            // whatever the host environment exports.
            assert_eq!(p.summarize(&request(Some("m"))).unwrap(), "ok", "{kind:?}");
            let got = seen.join().unwrap();
            for header in ["authorization", "x-api-key", "x-goog-api-key"] {
                assert!(got.header(header).is_none(), "{kind:?}: {header} sent");
            }
            let shown = describe_config(kind.as_str(), "m", &opts).join("\n");
            assert!(shown.contains("no key is sent to this base_url"), "{shown}");

            // If that server does want a key, say which setting provides it
            // rather than blaming a key that was never sent.
            let (server, _seen) = serve(401, &[], error_body("unauthorized"), Duration::ZERO);
            let mut opts = options(kind, &server);
            opts.api_key_env = String::new();
            let err = build(kind, &opts)
                .summarize(&request(Some("m")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("requires a key"), "{kind:?}: {err}");
            assert!(err.contains("api_key_env"), "{kind:?}: {err}");
            assert!(!err.contains("rejected the API key"), "{kind:?}: {err}");

            // The vendor's own endpoint keeps its conventional variable.
            let p = build(kind, &ApiOptions::default());
            assert_eq!(p.key_env.as_deref(), Some(p.flavor.default_key_env()));
        }
    }

    #[test]
    fn openai_error_inside_a_200_is_reported_as_the_providers_error() {
        let call = |body: Value| {
            let (server, _seen) = ok(body);
            provider(ProviderKind::OpenAi, &server).summarize_with_key(KEY, &request(Some("m")))
        };
        // OpenRouter's shape for an upstream failure.
        let err = call(
            json!({ "error": { "code": 502, "message": "Provider disconnected mid-stream" } }),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Provider disconnected mid-stream"), "{err}");
        assert!(err.contains("(code 502)"), "{err}");
        assert!(err.contains("in a 200 response"), "{err}");
        assert!(!err.contains("check `base_url`"), "the URL is fine: {err}");

        // `error: null` next to a real answer is not an error.
        let mut fine = success_body(ProviderKind::OpenAi, "ok");
        fine["error"] = Value::Null;
        assert_eq!(call(fine).unwrap(), "ok");

        // A partial answer the provider flags as failed is never a summary.
        let cut = json!({ "choices": [{
            "message": { "content": "The deploy pipeline has" },
            "finish_reason": "error",
        }] });
        let err = call(cut).unwrap_err().to_string();
        assert!(err.contains("the provider reported a failure"), "{err}");
        let cut = json!({ "choices": [{
            "message": { "content": "The deploy pipeline has" },
            "finish_reason": "stop",
            "error": { "code": 502, "message": "upstream died" },
        }] });
        let err = call(cut).unwrap_err().to_string();
        assert!(err.contains("upstream died"), "{err}");
        for reason in ["tool_calls", "function_call"] {
            let odd = json!({ "choices": [{
                "message": { "content": "text" }, "finish_reason": reason,
            }] });
            assert!(call(odd).is_err(), "{reason}");
        }
    }

    /// ureq would replay a redirected POST as a GET carrying `x-api-key` /
    /// `x-goog-api-key` to whatever host `Location` names.
    #[test]
    fn redirects_are_never_followed_and_the_key_stays_home() {
        for kind in ALL {
            for status in [301, 302, 303, 307, 308] {
                let sink = TcpListener::bind("127.0.0.1:0").unwrap();
                sink.set_nonblocking(true).unwrap();
                let sink_addr = sink.local_addr().unwrap();
                let mut redirect = canned(status, "");
                redirect.headers.push((
                    "location",
                    format!("http://{sink_addr}/login?next=/v1&token=SESSION-TOKEN"),
                ));
                let (server, seen) = serve_seq(vec![redirect]);
                let err = provider(kind, &server)
                    .summarize_with_key(KEY, &request(any_model(kind)))
                    .unwrap_err()
                    .to_string();

                let what = format!("{kind:?} {status}");
                assert!(
                    matches!(sink.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
                    "{what}: the redirect target was contacted"
                );
                assert_eq!(seen.all().len(), 1, "{what}");
                assert!(err.contains("redirect"), "{what}: {err}");
                assert!(err.contains(&format!("HTTP {status}")), "{what}: {err}");
                assert!(err.contains("`base_url`"), "{what}: {err}");
                // Where it points, without the query a login redirect carries.
                assert!(
                    err.contains(&format!("http://{sink_addr}")),
                    "{what}: {err}"
                );
                assert!(!err.contains("SESSION-TOKEN"), "{what}: {err}");
                assert_no_key("redirect", &err);
            }
        }
    }

    #[test]
    fn google_only_stop_is_a_complete_answer() {
        let partial = |reason: Option<&str>| {
            let mut candidate = json!({
                "content": { "parts": [{ "text": "The retriever combines BM25 with" }] },
            });
            if let Some(r) = reason {
                candidate["finishReason"] = json!(r);
            }
            json!({ "candidates": [candidate] })
        };
        let call = |body: Value| {
            let (server, _seen) = ok(body);
            provider(ProviderKind::Google, &server).summarize_with_key(KEY, &request(None))
        };

        for reason in [
            Some("OTHER"),
            Some("MALFORMED_RESPONSE"),
            Some("UNEXPECTED_TOOL_CALL"),
            Some("MISSING_THOUGHT_SIGNATURE"),
            Some("FINISH_REASON_UNSPECIFIED"),
            Some("A_REASON_ADDED_NEXT_YEAR"),
            None,
        ] {
            let err = call(partial(reason)).unwrap_err().to_string();
            assert!(
                err.contains("stopped before finishing"),
                "{reason:?}: {err}"
            );
            assert!(
                err.contains(&format!("finishReason={}", reason.unwrap_or("missing"))),
                "{reason:?}: {err}"
            );
        }
        // Filter verdicts are refusals, partial text or not.
        for reason in [
            "SAFETY",
            "LANGUAGE",
            "ESCALATION",
            "PUP_LIMITED_DISABLED",
            "IMAGE_SAFETY",
        ] {
            let err = call(partial(Some(reason))).unwrap_err().to_string();
            assert!(err.contains("declined to answer"), "{reason}: {err}");
        }
        assert!(
            call(partial(Some("MAX_TOKENS")))
                .unwrap_err()
                .to_string()
                .contains("output ceiling")
        );
        assert_eq!(
            call(partial(Some("STOP"))).unwrap(),
            "The retriever combines BM25 with"
        );
    }

    #[test]
    fn plain_http_is_refused_for_vendor_hosts_and_flagged_elsewhere() {
        for (flavor, url) in [
            (Flavor::Anthropic, "http://api.anthropic.com"),
            (Flavor::OpenAi, "http://api.openai.com/v1"),
            (Flavor::OpenAi, "http://eu.api.openai.com/v1"),
            (
                Flavor::Google,
                "http://generativelanguage.googleapis.com/v1beta",
            ),
        ] {
            let err = resolve_base_url(flavor, url)
                .err()
                .expect("a vendor host over plain http is a typo")
                .to_string();
            assert!(err.contains("must use https://"), "{url}: {err}");
        }
        // Local servers are what plain http is for.
        for url in [
            "http://127.0.0.1:1234/v1",
            "http://localhost:1234/v1",
            "http://LOCALHOST/v1",
            "http://[::1]:8080/v1",
            "http://127.9.9.9/v1",
        ] {
            assert!(
                !resolve_base_url(Flavor::OpenAi, url).unwrap().cleartext,
                "{url}"
            );
        }
        // Anything else over http is allowed (LAN GPU box, compose service)
        // but called out. Parsed, not prefix-matched.
        for url in [
            "http://gpu-box:8000/v1",
            "http://192.168.1.20:1234/v1",
            "http://127.0.0.1.evil.example/v1",
            "http://localhost.evil.example/v1",
        ] {
            assert!(
                resolve_base_url(Flavor::OpenAi, url).unwrap().cleartext,
                "{url}"
            );
        }
        assert!(
            !resolve_base_url(Flavor::OpenAi, "https://gpu-box:8000/v1")
                .unwrap()
                .cleartext
        );

        let lan = ApiOptions {
            base_url: "http://gpu-box:8000/v1".into(),
            ..ApiOptions::default()
        };
        let shown = describe_config("openai", "m", &lan).join("\n");
        assert!(
            shown.contains("WARNING: base_url is plain http://"),
            "{shown}"
        );
        let local = ApiOptions {
            base_url: "http://127.0.0.1:8000/v1".into(),
            ..ApiOptions::default()
        };
        assert!(
            !describe_config("openai", "m", &local)
                .join("\n")
                .contains("WARNING")
        );
    }

    #[test]
    fn redaction_handles_short_keys_and_escaped_keys() {
        // A placeholder key for a key-less local server must not shred the
        // message (and give itself away doing so)…
        assert_eq!(
            redact("connection to upstream refused; the request", "e"),
            "connection to upstream refused; the request"
        );
        assert_eq!(
            redact("http://127.0.0.1:1/v1 (os error 61)", "1"),
            // Only where the `1` stands alone; `v1` and `61` are words.
            "http://127.0.0.[redacted]:[redacted]/v1 (os error 61)"
        );
        assert_eq!(
            redact("request exceeds the context size", "x"),
            "request exceeds the context size"
        );
        // …while one echoed back as a token is still removed.
        assert_eq!(
            redact("bad key sk-1234 sent", "sk-1234"),
            "bad key [redacted] sent"
        );
        assert_eq!(redact("key=local;", "local"), "key=[redacted];");
        assert_eq!(redact("localhost", "local"), "localhost");

        // Keys that JSON escapes: every spelling goes.
        for (key, echoed) in [
            (
                "FAKEkey/revue+0123/abcdEFGH==",
                r"key FAKEkey\/revue+0123\/abcdEFGH== rejected",
            ),
            (
                "FAKEkey/revue+0123/abcdEFGH==",
                "key FAKEkey/revue+0123/abcdEFGH== rejected",
            ),
            (
                "FAKE\"key\\revue0123",
                r#"key FAKE\"key\\revue0123 rejected"#,
            ),
            (
                "FAKE\"key/revue0123",
                r#"key FAKE\"key\/revue0123 rejected"#,
            ),
        ] {
            let out = redact(echoed, key);
            assert_eq!(out, "key [redacted] rejected", "{key}");
        }
    }

    /// Every piece of server text that ends up in an error, on the 200 path
    /// and in headers as much as in error bodies.
    #[test]
    fn no_server_text_can_carry_the_key_into_an_error() {
        let slashed = "FAKEkey/revue+0123/abcdEFGH==";
        // An error object with no `message`, PHP-style escaping: quoted
        // whole, so the escaped spelling must be caught.
        let body = format!(
            r#"{{"error": {{"code": "bad_request", "info": "key {} rejected"}}}}"#,
            slashed.replace('/', "\\/")
        );
        for kind in ALL {
            let (server, _seen) = serve(400, &[], body.clone(), Duration::ZERO);
            let err = provider(kind, &server)
                .summarize_with_key(slashed, &request(Some("m")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("bad_request"), "detail kept: {err}");
            assert!(!err.contains("FAKEkey"), "{kind:?}: {err}");
            assert!(!err.contains("abcdEFGH"), "{kind:?}: {err}");
        }

        // `retry-after` is server text too: only seconds are ever shown.
        let (server, _seen) = serve_seq(
            (0..3)
                .map(|_| {
                    let mut c = canned(429, error_body("slow down"));
                    c.headers.push(("retry-after", KEY.to_string()));
                    c
                })
                .collect(),
        );
        let err = provider(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 429"), "{err}");
        assert!(!err.contains("retry-after"), "{err}");
        assert_no_key("retry-after", &err);

        let cases: Vec<(ProviderKind, Value)> = vec![
            (
                ProviderKind::OpenAi,
                json!({ "choices": [{ "message": { "content": null,
                    "refusal": format!("cannot comply, caller {KEY}") }, "finish_reason": "stop" }] }),
            ),
            (
                ProviderKind::OpenAi,
                json!({ "choices": [{ "message": { "content": "" }, "finish_reason": KEY }] }),
            ),
            (
                ProviderKind::OpenAi,
                json!({ "error": { "code": KEY, "message": format!("upstream said {KEY}") } }),
            ),
            (
                ProviderKind::Anthropic,
                json!({ "content": [], "stop_reason": "refusal", "stop_details": { "category": KEY } }),
            ),
            (
                ProviderKind::Anthropic,
                json!({ "content": [{ "type": "text", "text": "x" }], "stop_reason": KEY }),
            ),
            (
                ProviderKind::Google,
                json!({ "promptFeedback": { "blockReason": KEY } }),
            ),
            (
                ProviderKind::Google,
                json!({ "candidates": [{ "content": { "parts": [{ "text": "x" }] },
                    "finishReason": "OTHER", "finishMessage": format!("see {KEY}") }] }),
            ),
            (
                ProviderKind::Google,
                json!({ "candidates": [{ "finishReason": KEY }] }),
            ),
        ];
        for (kind, body) in cases {
            let (server, _seen) = ok(body.clone());
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("[redacted]"), "{kind:?} {body}: {err}");
            assert_no_key("200 path", &err);
        }

        // A key cut in two by the length limit must not survive in part.
        let long = format!("{}{KEY}", "x".repeat(190));
        let (server, _seen) = ok(json!({ "choices": [{ "message": { "content": null,
            "refusal": long }, "finish_reason": "stop" }] }));
        let err = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("m")))
            .unwrap_err()
            .to_string();
        assert_no_key("clipped refusal", &err);
        // 190 x + key, clipped at 200: clipping before redacting would
        // leave the first ten characters of the key.
        assert!(!err.contains(&KEY[..8]), "head of the key left: {err}");
        assert!(err.contains("[redacted]"), "{err}");
    }

    /// A host that drops packets used to hold the call for ureq's fixed 30s
    /// connect timeout, whatever `timeout_secs` said, then blame
    /// `timeout_secs`. The dead host is on the loopback interface — no
    /// packet leaves the machine: an unassigned 127/8 address where the OS
    /// drops them (macOS), else a listener whose accept queue is full where
    /// the OS drops SYNs then (Linux).
    #[test]
    fn connect_phase_is_bounded_by_the_request_timeout() {
        let probe = |addr: &std::net::SocketAddr| {
            matches!(
                std::net::TcpStream::connect_timeout(addr, Duration::from_millis(200)),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut
            )
        };
        let unassigned: std::net::SocketAddr = "127.0.0.2:81".parse().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut held = Vec::new();
        let addr = if probe(&unassigned) {
            unassigned
        } else {
            let full = listener.local_addr().unwrap();
            let mut saturated = false;
            for _ in 0..400 {
                match std::net::TcpStream::connect_timeout(&full, Duration::from_millis(200)) {
                    Ok(stream) => held.push(stream),
                    Err(e) => {
                        saturated = e.kind() == std::io::ErrorKind::TimedOut;
                        break;
                    }
                }
            }
            if !saturated {
                // Neither trick yields a silent host on this platform.
                eprintln!("connect_phase_is_bounded_by_the_request_timeout: skipped");
                return;
            }
            full
        };

        for kind in ALL {
            let req = SummarizeRequest {
                timeout: Duration::from_millis(400),
                ..request(any_model(kind))
            };
            let started = Instant::now();
            let err = provider(kind, &format!("http://{addr}"))
                .summarize_with_key(KEY, &req)
                .unwrap_err()
                .to_string();
            let elapsed = started.elapsed();
            // A 400 ms budget: anything near ureq's fixed 30 s is the bug.
            assert!(
                elapsed < Duration::from_secs(2),
                "{kind:?}: waited {elapsed:?}"
            );
            assert!(err.contains("could not connect"), "{kind:?}: {err}");
            assert!(!err.contains("raise `timeout_secs`"), "{kind:?}: {err}");
            assert_no_key("connect timeout", &err);
        }
        drop(held);
    }
    // ── truncated answers and reasoning tags ─────────────────────────────

    /// Compatible servers have their own words for "cut short"; an answer
    /// whose finish reason is not a natural stop is never a summary.
    #[test]
    fn openai_only_a_natural_stop_is_a_complete_answer() {
        let partial = |reason: Value| {
            json!({ "choices": [{
                "message": { "content": "- fact 000\n- fact 001 about pr" },
                "finish_reason": reason,
            }] })
        };
        let call = |body: Value| {
            let (server, _seen) = ok(body);
            provider(ProviderKind::OpenAi, &server).summarize_with_key(KEY, &request(Some("m")))
        };
        for reason in [
            "model_length",                 // Mistral: context window reached
            "insufficient_system_resource", // DeepSeek: interrupted
            "aborted",
            "cancelled",
            "timeout",
            "a_reason_invented_next_year",
        ] {
            let err = call(partial(json!(reason))).unwrap_err().to_string();
            assert!(err.contains("discarded"), "{reason}: {err}");
            assert!(!err.contains("fact 000"), "{reason}: partial text quoted");
        }
        // Missing altogether, and the upper-case spellings some gateways use.
        assert!(
            call(partial(Value::Null))
                .unwrap_err()
                .to_string()
                .contains("no finish_reason")
        );
        assert!(
            call(partial(json!("MAX_TOKENS")))
                .unwrap_err()
                .to_string()
                .contains("output ceiling")
        );
        assert!(
            call(partial(json!("LENGTH")))
                .unwrap_err()
                .to_string()
                .contains("output ceiling")
        );
        assert!(call(partial(json!("ERROR"))).is_err());
        assert!(
            call(partial(json!("model_length")))
                .unwrap_err()
                .to_string()
                .contains("context window")
        );

        for reason in [
            "stop",
            "STOP",
            "eos",
            "eos_token",
            "end_turn",
            "stop_sequence",
        ] {
            let out = call(partial(json!(reason))).unwrap();
            assert!(out.ends_with("fact 001 about pr"), "{reason}");
        }
    }

    /// The host ICM classifies must be the host the HTTP client connects
    /// to. ureq's URL parser reads `\` as `/` and `%2e` as `.`: a third
    /// party could pass for the vendor (and be sent the vendor's default
    /// key), or the vendor over plain http pass for a third party.
    #[test]
    fn base_url_host_cannot_be_disguised() {
        for (flavor, url) in [
            // The client would connect to `attacker.example`.
            (
                Flavor::OpenAi,
                r"https://attacker.example\.api.openai.com/v1",
            ),
            (Flavor::OpenAi, r"https://127.0.0.1:8443\.api.openai.com/v1"),
            (
                Flavor::OpenAi,
                r"https://attacker.example\@api.openai.com/v1",
            ),
            // Same server as the vendor's, spelled so as not to look it.
            (Flavor::OpenAi, "http://api%2eopenai.com/v1"),
            (Flavor::OpenAi, "http://api.openai.com%2e/v1"),
            (Flavor::Anthropic, "http://api%2Eanthropic.com"),
            // Nothing a host name is made of.
            (Flavor::OpenAi, "https://api.openai.com /v1"),
            (Flavor::OpenAi, "https://api.openai.com\t/v1"),
            (Flavor::OpenAi, "https://ａpi.openai.com/v1"),
            (Flavor::OpenAi, "https://host:port/v1"),
            (Flavor::OpenAi, "https://host:/v1"),
            (Flavor::OpenAi, "https://[::1/v1"),
            (Flavor::OpenAi, "https://[::1]x/v1"),
            (Flavor::OpenAi, "https://[zz]/v1"),
        ] {
            assert!(resolve_base_url(flavor, url).is_err(), "accepted: {url}");
        }

        // A trailing dot is the same server: the vendor's rules apply.
        for (flavor, url) in [
            (Flavor::OpenAi, "http://api.openai.com./v1"),
            (Flavor::Anthropic, "http://api.anthropic.com."),
            (
                Flavor::Google,
                "http://generativelanguage.googleapis.com./v1beta",
            ),
        ] {
            let err = resolve_base_url(flavor, url)
                .err()
                .unwrap_or_else(|| panic!("accepted: {url}"))
                .to_string();
            assert!(err.contains("must use https://"), "{url}: {err}");
        }
        assert!(
            resolve_base_url(Flavor::OpenAi, "https://api.openai.com./v1")
                .unwrap()
                .vendor_host
        );

        // Ordinary forms still pass, classified as before.
        for (url, vendor) in [
            ("https://eu.api.openai.com/v1", true),
            ("https://API.OpenAI.com:443/v1", true),
            ("https://openrouter.ai/api/v1", false),
            ("http://localhost:1234/v1", false),
            ("http://127.0.0.1:8000/v1", false),
            ("http://[::1]:8080/v1", false),
            ("https://my-gateway.internal.example:8443/openai/v1", false),
        ] {
            let e = resolve_base_url(Flavor::OpenAi, url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(e.vendor_host, vendor, "{url}");
        }

        // The disguised host never gets the vendor's default key variable.
        let spoofed = ApiOptions {
            base_url: r"https://127.0.0.1:8443\.api.openai.com/v1".into(),
            ..ApiOptions::default()
        };
        let shown = describe_config("openai", "", &spoofed).join("\n");
        assert!(shown.contains("invalid:"), "{shown}");
        assert!(!shown.contains("OPENAI_API_KEY"), "{shown}");
    }

    /// `?key=…` is how Gemini's own curl examples pass the key; pasted into
    /// `base_url` it would be printed by `icm config` and by every error.
    #[test]
    fn base_url_with_a_query_or_fragment_is_refused_without_echo() {
        for url in [
            "https://generativelanguage.googleapis.com/v1beta?key=fake-gemini-key-CCCC3333",
            "http://127.0.0.1:9/v1beta?key=fake-gemini-key-CCCC3333",
            "https://gw.example/v1?api-key=fake-gemini-key-CCCC3333&x=1",
            "https://gw.example/v1#fake-gemini-key-CCCC3333",
        ] {
            let err = resolve_base_url(Flavor::Google, url)
                .err()
                .unwrap_or_else(|| panic!("accepted: {url}"))
                .to_string();
            assert!(err.contains("query string"), "{err}");
            assert!(!err.contains("fake-gemini-key"), "value echoed: {err}");
            let opts = ApiOptions {
                base_url: url.into(),
                ..ApiOptions::default()
            };
            let shown = describe_config("google", "", &opts).join("\n");
            assert!(
                !shown.contains("fake-gemini-key"),
                "icm config shows it: {shown}"
            );
        }
    }

    #[test]
    fn redaction_covers_the_encodings_a_server_may_echo_the_key_in() {
        let key = "fk-AbC/dEf+gHi=jKl0123+/xyZ==";
        let json1 = serde_json::to_string(key).unwrap();
        let json1 = json1.trim_matches('"');
        let json2 = serde_json::to_string(json1).unwrap();
        let json2 = json2.trim_matches('"').to_string();
        let quoted_key = "FAKE\"key\\revue0123";
        let quoted1 = serde_json::to_string(quoted_key).unwrap();
        let quoted2 = serde_json::to_string(quoted1.trim_matches('"')).unwrap();
        let quoted3 = serde_json::to_string(quoted2.trim_matches('"')).unwrap();
        let cases: Vec<(&str, String)> = vec![
            (
                key,
                "credential fk-AbC%2FdEf%2BgHi%3DjKl0123%2B%2FxyZ%3D%3D".into(),
            ),
            (
                key,
                "credential fk-abc%2fdef%2bghi%3djkl0123%2b%2fxyz%3d%3d".into(),
            ),
            (key, format!("upstream said {json2}")),
            (key, r"credential fk-AbC/dEf+gHi=jKl0123+/xyZ==".into()),
            (key, r"credential fk-AbC\/dEf+gHi=jKl0123+\/xyZ==".into()),
            (quoted_key, "Bearer FAKE&quot;key\\revue0123".into()),
            (quoted_key, format!("nested {}", quoted2.trim_matches('"'))),
            (
                quoted_key,
                format!("nested twice {}", quoted3.trim_matches('"')),
            ),
        ];
        for (key, echoed) in cases {
            let out = redact(&echoed, key);
            assert!(
                out.contains("[redacted]"),
                "not redacted: {echoed} -> {out}"
            );
            for fragment in [
                "AbC",
                "dEf",
                "gHi",
                "xyZ",
                "revue0123",
                "%2F",
                "%2f",
                "u002f",
            ] {
                assert!(!out.contains(fragment), "{fragment} left in: {out}");
            }
        }

        // End to end: a gateway relaying the upstream body inside a string
        // of an error object that has no `message`.
        let upstream = json!({ "error": { "message": format!("credential {key}") } }).to_string();
        let body = json!({ "error": { "upstream_body": upstream } }).to_string();
        for kind in ALL {
            let (server, _seen) = serve(403, &[], body.clone(), Duration::ZERO);
            let err = provider(kind, &server)
                .summarize_with_key(key, &request(Some("m")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("upstream_body"), "detail kept: {err}");
            for fragment in ["fk-AbC", "dEf", "xyZ"] {
                assert!(!err.contains(fragment), "{kind:?}: {err}");
            }
        }
    }

    /// The last line of defense — the pass over every error leaving the
    /// module — on its own: here the key is not in any server text, so no
    /// call site's quoting can be what removes it.
    #[test]
    fn the_final_redaction_pass_covers_text_no_call_site_quotes() {
        let (server, _seen) = serve(400, &[], error_body("bad request"), Duration::ZERO);
        let err = provider(ProviderKind::OpenAi, &server)
            // A model name that happens to be the key: echoed by ICM itself.
            .summarize_with_key(KEY, &request(Some(KEY)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("model=[redacted]"), "{err}");
        assert_no_key("final pass", &err);
    }

    /// A placeholder key for a key-less server is not a secret, and must
    /// not eat ICM's own words: "rejected the API [redacted]".
    #[test]
    fn a_placeholder_key_does_not_garble_icms_own_messages() {
        for placeholder in ["key", "model", "openai", "a", "1", "0"] {
            let (server, _seen) = serve(401, &[], error_body("no"), Duration::ZERO);
            let err = provider(ProviderKind::OpenAi, &server)
                .summarize_with_key(placeholder, &request(Some("a-ctx")))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("openai rejected the API key (HTTP 401)"),
                "{placeholder}: {err}"
            );
            assert!(err.contains("non-revoked key"), "{placeholder}: {err}");
            assert!(!err.contains("[redacted]"), "{placeholder}: {err}");

            let (server, _seen) = serve(400, &[], error_body("context too small"), Duration::ZERO);
            let err = provider(ProviderKind::OpenAi, &server)
                .summarize_with_key(placeholder, &request(Some("a-ctx")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("model=a-ctx"), "{placeholder}: {err}");
            assert!(err.contains(&server), "base_url kept readable: {err}");
        }
    }

    /// A compatible server with a small context window rejects prompt + cap.
    /// The floor used to make that unfixable: `max_tokens = 1` still sent
    /// 8192. Now the floor is lower there, and gives way once.
    #[test]
    fn a_small_context_server_can_be_used_by_lowering_max_tokens() {
        let too_long = error_body(
            "This model's maximum context length is 2048 tokens. However, you requested \
             2278 tokens (230 in the messages, 2048 in the completion).",
        );
        let (server, seen) = serve_seq(vec![
            canned(400, too_long.clone()),
            canned(200, success_body(ProviderKind::OpenAi, "ok").to_string()),
        ]);
        let req = SummarizeRequest {
            max_tokens: 100,
            ..request(Some("meta-llama/Meta-Llama-3-8B-Instruct"))
        };
        let out = provider(ProviderKind::OpenAi, &server).summarize_with_key(KEY, &req);
        assert_eq!(out.unwrap(), "ok");
        let sent = seen.all();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].json()["max_tokens"], 2048, "the floor first");
        assert_eq!(
            sent[1].json()["max_tokens"],
            400,
            "then 4x max_tokens, no floor"
        );

        // Still too much for the server: one retry, then its error.
        let (server, seen) = serve_seq(vec![
            canned(400, too_long.clone()),
            canned(400, too_long.clone()),
            canned(200, success_body(ProviderKind::OpenAi, "never").to_string()),
        ]);
        let err = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &req)
            .unwrap_err()
            .to_string();
        assert_eq!(seen.all().len(), 2, "one retry without the floor, no more");
        assert!(err.contains("HTTP 400"), "{err}");
        // The cap now follows `max_tokens`, so lowering it is real advice.
        assert!(err.contains("the value sent (400)"), "{err}");
        assert!(err.contains("lower `max_tokens`"), "{err}");
        assert!(!err.contains("would not change it"), "{err}");

        // On a vendor host the floor stays: the limits there are known.
        let (server, seen) = serve_seq(vec![
            canned(400, error_body("max_tokens: 8192 > 4096")),
            canned(
                200,
                success_body(ProviderKind::Anthropic, "never").to_string(),
            ),
        ]);
        let err = vendor_default(ProviderKind::Anthropic, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap_err()
            .to_string();
        assert_eq!(seen.all().len(), 1);
        assert!(err.contains("the smallest ICM sends"), "{err}");
        assert!(err.contains("would not change it"), "{err}");
    }

    #[test]
    fn redirect_to_the_same_host_shows_the_path_and_the_right_advice() {
        let redirected = |location: String| {
            let mut redirect = canned(308, "");
            redirect.headers.push(("location", location));
            let (server, _seen) = serve_seq(vec![redirect]);
            let err = provider(ProviderKind::OpenAi, &server)
                .summarize_with_key(KEY, &request(Some("m")))
                .unwrap_err()
                .to_string();
            (server, err)
        };

        // Relative: an auth wall or a rewritten prefix on the same server.
        let (_, err) = redirected("/oauth2/sign_in?rd=%2Fv1&state=SESSION-TOKEN".into());
        assert!(err.contains("/oauth2/sign_in on the same host"), "{err}");
        assert!(err.contains("login page"), "{err}");
        assert!(!err.contains("http:// → https://"), "{err}");
        assert!(!err.contains("SESSION-TOKEN"), "{err}");

        // Absolute, same host and scheme: the path is what tells it apart.
        let (target, advice) = redirect_target(
            "http://gateway.example:8080/openai/v1/chat/completions?x=SESSION-TOKEN",
            "http://gateway.example:8080/v1",
            KEY,
        );
        assert_eq!(
            target,
            "http://gateway.example:8080/openai/v1/chat/completions"
        );
        assert!(
            advice.contains("path in `base_url` is probably wrong"),
            "{advice}"
        );

        // Another host, or another scheme: the original advice.
        let (target, advice) = redirect_target(
            "https://user:pw@login.corp.example/sso/start?token=SESSION-TOKEN",
            "http://gateway.example/v1",
            KEY,
        );
        assert_eq!(target, "https://login.corp.example/sso/start");
        assert!(advice.contains("final URL"), "{advice}");
        let (target, advice) = redirect_target(
            "https://gateway.example/v1",
            "http://gateway.example/v1",
            KEY,
        );
        assert_eq!(target, "https://gateway.example/v1");
        assert!(advice.contains("http:// → https://"), "{advice}");
        let (target, advice) =
            redirect_target("//other.example/v1", "http://gateway.example/v1", KEY);
        assert_eq!(target, "//other.example/v1");
        assert!(advice.contains("final URL"), "{advice}");
        assert_eq!(
            redirect_target("", "http://h/v1", KEY).0,
            "an unnamed location"
        );
    }

    /// An error reported in a 200, for the three formats; and what is not
    /// one.
    #[test]
    fn an_error_reported_in_a_200_is_never_blamed_on_base_url() {
        for kind in ALL {
            let (server, _seen) = ok(json!({ "type": "error", "error": {
                "code": 503, "message": "upstream overloaded" } }));
            let err = provider(kind, &server)
                .summarize_with_key(KEY, &request(any_model(kind)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("upstream overloaded"), "{kind:?}: {err}");
            assert!(err.contains("in a 200 response"), "{kind:?}: {err}");
            assert!(!err.contains("check `base_url`"), "{kind:?}: {err}");

            // Members that look like "no error" next to a real answer.
            for harmless in [json!(""), json!({}), json!(false), Value::Null] {
                let mut body = success_body(kind, "fine");
                body["error"] = harmless.clone();
                let (server, _seen) = ok(body);
                let out =
                    provider(kind, &server).summarize_with_key(KEY, &request(any_model(kind)));
                assert_eq!(out.unwrap(), "fine", "{kind:?} error={harmless}");
            }
        }

        // The provider cutting its own answer short is its failure too.
        for choice in [
            json!({ "message": { "content": "The deploy pipeline has" }, "finish_reason": "error" }),
            json!({ "message": { "content": "The deploy pipeline has" }, "finish_reason": "stop",
                    "error": { "code": 502, "message": "Provider disconnected mid-stream" } }),
        ] {
            let (server, _seen) = ok(json!({ "choices": [choice] }));
            let err = provider(ProviderKind::OpenAi, &server)
                .summarize_with_key(KEY, &request(Some("m")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("the provider reported a failure"), "{err}");
            assert!(err.contains("retry later"), "{err}");
            assert!(!err.contains("check `base_url`"), "the URL is fine: {err}");
            assert!(
                !err.contains("deploy pipeline"),
                "partial text quoted: {err}"
            );
        }
        // An empty `error` member on the choice is not a failure.
        let (server, _seen) = ok(json!({ "choices": [{
            "message": { "content": "fine" }, "finish_reason": "stop", "error": {} }] }));
        let out =
            provider(ProviderKind::OpenAi, &server).summarize_with_key(KEY, &request(Some("m")));
        assert_eq!(out.unwrap(), "fine");
    }

    #[test]
    fn openai_strips_a_leading_reasoning_block() {
        let answer_of = |text: &'static str, input_mentions_tag: bool| match split_reasoning(
            text,
            input_mentions_tag,
        ) {
            Reasoning::Answer(a) => Ok(a),
            Reasoning::Unfinished => Err("unfinished"),
            Reasoning::Ambiguous => Err("ambiguous"),
        };
        assert_eq!(answer_of("<think>a</think>b", false), Ok("b"));
        assert_eq!(answer_of("  <THINK>\na\n</Think>\n\nb", false), Ok("\n\nb"));
        assert_eq!(answer_of("<think>never closed", false), Err("unfinished"));
        assert_eq!(answer_of("plain", false), Ok("plain"));
        // A mention in the body of an answer is not a scratchpad.
        let mention = "Thinking models emit <think>…</think> blocks (#253).";
        assert_eq!(answer_of(mention, false), Ok(mention));
        assert_eq!(answer_of(mention, true), Ok(mention));
        // A leading block is only cut when nothing says it could be the
        // answer: not when the input talks about the tag, not when a tag
        // is left after the cut.
        assert_eq!(answer_of("<think>a</think>b", true), Err("ambiguous"));
        assert_eq!(
            answer_of("<think>a</think>b</think>c", false),
            Err("ambiguous")
        );
        // A lone closing tag is never cut at, whatever the input says.
        assert_eq!(answer_of("a\n</think>b", false), Err("ambiguous"));
        assert_eq!(answer_of("a\n</THINK>b", false), Err("ambiguous"));

        let answer = "- The project uses Postgres on port 5433.";
        let content = format!(
            "<think>\nThe user wants a merge.\n- maybe mention MySQL\n</think>\n\n{answer}"
        );
        let (server, _seen) = ok(success_body(ProviderKind::OpenAi, &content));
        let out = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("qwen3")))
            .unwrap();
        assert_eq!(out, answer, "reasoning stored as the summary");

        let (server, _seen) = ok(success_body(ProviderKind::OpenAi, mention));
        let out = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("m")))
            .unwrap();
        assert_eq!(out, mention);

        // Reasoning that never ends is not an answer, whatever the server
        // says about having stopped normally.
        let (server, _seen) = ok(success_body(
            ProviderKind::OpenAi,
            "<think>\nstill thinking",
        ));
        let err = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("qwen3")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unfinished <think>"), "{err}");
    }

    /// A complete answer that *quotes* `</think>` — the memories being
    /// merged are about that tag — must not be cut at the tag: the half
    /// that survived was stored and the originals deleted. The first fix
    /// only looked for the literal tag in the input; a memory that spells
    /// it any other way reopened the hole.
    #[test]
    fn openai_never_cuts_an_answer_that_quotes_the_think_tag() {
        const ANSWER: &str = "- The project uses Postgres 16\n- Postgres listens on port 5433\n\
            - Issue #253: qwen3 writes its visible answer only after `</think>`, so a \
            400-token budget left it empty\n- Fix for #253: send think=false to Ollama";

        // On OpenAI's own API reasoning never comes in `content`: the
        // answer is returned whole, whatever the input says.
        let (server, _seen) = ok(success_body(ProviderKind::OpenAi, ANSWER));
        let out = vendor_default(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(None))
            .unwrap();
        assert_eq!(out, ANSWER, "the summary was cut at the quoted tag");

        // On a compatible server it is refused — the caller keeps the
        // originals — rather than cut, however the input spells the tag,
        // and even when the input does not mention it at all.
        for input in [
            "memory: the answer comes after </think>",
            "memory: the answer comes after &lt;/think&gt;",
            "memory: the answer comes after </THINK>",
            "memory: the answer comes after \\u003c/think\\u003e",
            "memory: qwen3 only answers after its closing think tag",
            "memory: nothing about tags here",
        ] {
            let req = SummarizeRequest {
                prompt: input,
                ..request(Some("m"))
            };
            let (server, _seen) = ok(success_body(ProviderKind::OpenAi, ANSWER));
            let err = provider(ProviderKind::OpenAi, &server)
                .summarize_with_key(KEY, &req)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("cannot be told from a quotation"),
                "{input}: {err}"
            );
            assert!(
                !err.contains("400-token budget"),
                "partial text quoted: {err}"
            );
        }

        // A leading block in the answer, and an input that talks about the
        // tag in an escaped spelling: refused as well.
        let req = SummarizeRequest {
            prompt: "memory: &lt;think&gt; blocks eat the budget",
            ..request(Some("m"))
        };
        let (server, _seen) = ok(success_body(
            ProviderKind::OpenAi,
            "<think> blocks eat the budget; the answer follows </think>. Use think=false.",
        ));
        assert!(
            provider(ProviderKind::OpenAi, &server)
                .summarize_with_key(KEY, &req)
                .is_err()
        );
    }

    #[test]
    fn anthropic_only_a_natural_stop_is_a_complete_answer() {
        let partial = |stop: Value| {
            json!({
                "content": [{ "type": "text", "text": "- fact one\n- fact two is cut mid-sen" }],
                "stop_reason": stop,
            })
        };
        let call = |body: Value| {
            let (server, _seen) = ok(body);
            provider(ProviderKind::Anthropic, &server).summarize_with_key(KEY, &request(None))
        };

        let err = call(partial(json!("model_context_window_exceeded")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("context window"), "{err}");
        assert!(err.contains("reduce the input"), "{err}");
        assert!(!err.contains("output ceiling"), "wrong advice: {err}");

        for stop in ["pause_turn", "tool_use", "some_future_value"] {
            let err = call(partial(json!(stop))).unwrap_err().to_string();
            assert!(err.contains("stopped before finishing"), "{stop}: {err}");
            assert!(
                err.contains(&format!("stop_reason={stop}")),
                "{stop}: {err}"
            );
        }

        // A gateway that drops the field cannot say the answer is whole:
        // refused, like the other two formats (it used to be accepted).
        let err = call(partial(Value::Null)).unwrap_err().to_string();
        assert!(err.contains("no stop_reason"), "{err}");
        let mut missing = partial(Value::Null);
        missing.as_object_mut().unwrap().remove("stop_reason");
        assert!(call(missing).is_err());

        for stop in ["end_turn", "stop_sequence"] {
            let out = call(partial(json!(stop))).unwrap();
            assert!(out.ends_with("cut mid-sen"), "{stop}: {out}");
        }
    }

    /// Ollama: only an answer the server says is finished counts, the
    /// output cap has the same headroom as on the other providers, and the
    /// context window is sized on the prompt instead of being left at a
    /// default that truncates a full consolidation prompt in silence.
    #[test]
    fn ollama_only_accepts_a_finished_answer_about_the_whole_prompt() {
        let call = |body: Value| {
            let (host, seen) = ok(body);
            let out =
                super::super::OllamaSummarizer { host }.summarize(&request(Some("qwen2.5:7b")));
            (out, seen.join().unwrap())
        };
        let (out, sent) = call(json!({
            "response": "- fact 000\n- fact 001 about pr", "done": true, "done_reason": "length",
        }));
        let err = out.unwrap_err().to_string();
        assert!(err.contains("stopped before finishing"), "{err}");
        assert!(err.contains("done_reason=length"), "{err}");
        assert!(err.contains("num_predict was 2048"), "{err}");
        assert!(!err.contains("fact 000"), "{err}");

        // The request: headroom over the 400-token budget, a window that
        // holds the prompt, and no silent truncation.
        let body = sent.json();
        assert_eq!(sent.path, "/api/generate");
        assert_eq!(
            body["options"]["num_predict"], 2048,
            "4x the budget, minimum 2048"
        );
        let num_ctx = body["options"]["num_ctx"].as_u64().unwrap() as usize;
        assert!(num_ctx >= PROMPT.len() / 3 + 2048, "num_ctx = {num_ctx}");
        assert_eq!(body["truncate"], false);
        assert_eq!(body["shift"], false);
        assert_eq!(super::super::ollama_num_predict(400), 2048);
        assert_eq!(super::super::ollama_num_predict(1000), 4000);
        // A full consolidation prompt (20 000 characters) does not fit the
        // 4096-token default: the window asked for must.
        assert!(super::super::ollama_num_ctx(20_000, 2048) >= 20_000 / 3 + 2048);
        assert_eq!(super::super::ollama_num_ctx(10, 2048) % 1024, 0);
        assert!(super::super::ollama_num_ctx(10_000_000, 2048) <= 32_768);

        for cut in [
            json!({ "response": "half an ans", "done": false }),
            json!({ "response": "half an ans" }),
            json!({ "response": "half an ans", "done": true, "done_reason": "load" }),
            // No `done_reason` (older servers) and the whole cap used.
            json!({ "response": "half an ans", "done": true, "eval_count": 2048 }),
        ] {
            let (out, _) = call(cut.clone());
            assert!(out.is_err(), "accepted: {cut}");
        }

        // The window was full: the model did not read the whole prompt.
        let (out, sent) = call(json!({
            "response": "summary of the part that got in", "done": true, "done_reason": "stop",
            "prompt_eval_count": 4000, "eval_count": 96,
        }));
        let sent_ctx = sent.json()["options"]["num_ctx"].as_u64().unwrap();
        assert!(sent_ctx <= 4096, "test premise: {sent_ctx}");
        let err = out.unwrap_err().to_string();
        assert!(err.contains("filled its context window"), "{err}");

        for whole in [
            json!({ "response": "whole answer", "done": true, "done_reason": "stop",
                    "prompt_eval_count": 40, "eval_count": 12 }),
            json!({ "response": "whole answer", "done": true, "done_reason": "STOP" }),
            json!({ "response": "whole answer", "done": true }),
        ] {
            assert_eq!(call(whole).0.unwrap(), "whole answer");
        }
    }

    /// The host check added against disguised hosts refused hosts that are
    /// perfectly valid: docker-compose service names, and IPv6 addresses
    /// not written in their shortest form.
    #[test]
    fn base_url_accepts_the_valid_hosts_the_host_check_refused() {
        for url in [
            "http://llm_server:8000/v1",
            "http://vllm_1/v1",
            "https://my_gateway.internal.example/v1",
        ] {
            let e = resolve_base_url(Flavor::OpenAi, url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert!(!e.vendor_host, "{url}");
        }
        for url in [
            "http://[0:0:0:0:0:0:0:1]:8080/v1",
            "http://[0000:0000:0000:0000:0000:0000:0000:0001]/v1",
            "http://[::1]:8080/v1",
        ] {
            let e = resolve_base_url(Flavor::OpenAi, url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert!(!e.cleartext, "{url} is loopback");
        }
        let e = resolve_base_url(
            Flavor::OpenAi,
            "http://[2001:0db8:0000:0000:0000:0000:0000:0001]/v1",
        )
        .unwrap();
        assert!(e.cleartext);

        // Still refused, and for the stated reason.
        for url in ["http://host:70000/v1", "http://host:99999999999/v1"] {
            let err = resolve_base_url(Flavor::OpenAi, url)
                .err()
                .unwrap_or_else(|| panic!("accepted: {url}"))
                .to_string();
            assert!(err.contains("between 0 and 65535"), "{url}: {err}");
        }
        assert!(
            resolve_base_url(Flavor::OpenAi, r"https://evil.example\_.api.openai.com/v1").is_err()
        );
    }

    /// `"/"` is what a template like `"${LLM_BASE_URL}/"` renders to with
    /// an empty variable. It was read as "no base_url": the vendor's own
    /// endpoint, to which the key named for the gateway was then sent.
    #[test]
    fn base_url_made_of_slashes_is_malformed_not_the_vendor_default() {
        for url in ["/", "//", " / ", "///"] {
            for flavor in [Flavor::Anthropic, Flavor::OpenAi, Flavor::Google] {
                let err = resolve_base_url(flavor, url)
                    .err()
                    .unwrap_or_else(|| panic!("{url:?} accepted for {flavor:?}"))
                    .to_string();
                assert!(err.contains("must start with http:// or https://"), "{err}");
            }
            let opts = ApiOptions {
                api_key_env: "GATEWAY_KEY".into(),
                base_url: url.into(),
                workspace_id: String::new(),
            };
            assert!(ApiSummarizer::new(ProviderKind::OpenAi, &opts).is_err());
        }
        // Really empty is still the default.
        for url in ["", "   "] {
            assert!(resolve_base_url(Flavor::OpenAi, url).unwrap().vendor_host);
        }
    }

    /// Server text ends up on a terminal and in log files. Escape
    /// sequences in it could erase the error line or rewrite the one above.
    #[test]
    fn server_text_cannot_put_control_characters_in_an_error() {
        let hostile = "bad\u{1b}[2K\u{1b}[1A\u{7}request\u{9b}31m \\u001b[2K and \r\n more";
        let no_controls = |context: &str, err: &str| {
            assert!(
                !err.chars().any(char::is_control),
                "{context}: control character left in {err:?}"
            );
        };
        for kind in ALL {
            for status in [400, 403, 429, 503] {
                let (server, _seen) = serve_seq(
                    (0..3)
                        .map(|_| canned(status, error_body(hostile)))
                        .collect(),
                );
                let err = provider(kind, &server)
                    .summarize_with_key(KEY, &request(Some("m")))
                    .unwrap_err()
                    .to_string();
                assert!(err.contains("request"), "the text itself is kept: {err}");
                no_controls("error body", &err);
            }
        }
        // The 200 path and a redirect target.
        let (server, _seen) = ok(json!({ "choices": [{
            "message": { "content": null, "refusal": hostile }, "finish_reason": "stop" }] }));
        let err = provider(ProviderKind::OpenAi, &server)
            .summarize_with_key(KEY, &request(Some("m")))
            .unwrap_err()
            .to_string();
        no_controls("refusal", &err);
        let (target, _) = redirect_target("https://x.example/\u{1b}[2Kpath", "http://h/v1", KEY);
        no_controls("redirect", &target);
        assert_eq!(printable("a\u{1b}b\tc\n\nd"), "a b c d");
    }
}
