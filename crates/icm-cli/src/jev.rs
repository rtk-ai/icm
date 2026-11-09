//! Optional reranking of `icm recall` results with a System One decision
//! model over the `/v1/systemone` wire protocol: TypeSafe's hosted Jev, or a
//! Laya server (`laya-serve`, open weights) speaking the same protocol.
//!
//! Enabled by `[recall] reranker`: `"jev"` / `"jev:<model>"` calls
//! api.typesafe.ai with `TYPESAFE_API_KEY`; `"laya"` / `"laya:<model>"` calls
//! a Laya server, `http://127.0.0.1:8000/v1/systemone` unless
//! `reranker_url` says otherwise. Either way the query and the candidate
//! memory text are sent to that server. Empty (the default): off.
//!
//! Every candidate is scored in one batched request, each as an independent
//! yes/no (`noul`) question; the score is the probability of "yes".
//!
//! The prompt strings and the batching are ported from memsearch's
//! `jev_reranker.py` (https://github.com/zilliztech/memsearch), itself
//! adapted from https://docs.typesafe.ai/cookbooks/rerank_typesafe, under
//! this notice:
//!
//! MIT License
//!
//! Copyright (c) 2025 Zilliz Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the
//! "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish,
//! distribute, sublicense, and/or sell copies of the Software, and to permit
//! persons to whom the Software is furnished to do so, subject to the
//! following conditions:
//!
//! The above copyright notice and this permission notice shall be included
//! in all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
//! OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
//! NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
//! DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
//! OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
//! USE OR OTHER DEALINGS IN THE SOFTWARE.

use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::config::RecallConfig;

const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const LAYA_ENDPOINT: &str = "http://127.0.0.1:8000/v1/systemone";
const JEV_DEFAULT_MODEL: &str = "jev-latest";

const INSTRUCTIONS: &str = "The query asks about information recorded in project memory. \
Could the candidate passage be a source for the answer — does it state the specific fact, \
decision, procedure, or event the query asks about?";
const CRITERIA_TRUE: &str = "The candidate passage states or establishes the specific \
information needed to answer the query, or a necessary supporting fact for a query requiring \
multiple passages.";
const CRITERIA_FALSE: &str = "The candidate passage is merely on a similar topic or project; \
it does not supply the specific information the query requires.";

/// A configured reranker: where to send the request and with what.
#[derive(Debug, Clone, PartialEq)]
pub struct Reranker {
    /// `"Jev"` or `"Laya"`, for messages.
    pub name: &'static str,
    pub endpoint: String,
    /// `None` lets a Laya server pick its checkpoint.
    pub model: Option<String>,
    /// Bearer token. Required for Jev, optional for Laya.
    pub api_key: Option<String>,
    pub timeout: Duration,
}

impl Reranker {
    /// The reranker `[recall]` selects, or `None` when `reranker` is empty.
    /// Fails on an unknown kind and on Jev without its key: both are
    /// configuration mistakes the user must see, not a reason to fall back.
    pub fn from_config(cfg: &RecallConfig) -> Result<Option<Self>> {
        let spec = cfg.reranker.trim();
        if spec.is_empty() {
            return Ok(None);
        }
        let (kind, model) = match spec.split_once(':') {
            Some((k, m)) => (k.trim(), Some(m.trim()).filter(|m| !m.is_empty())),
            None => (spec, None),
        };
        let (name, default_url, default_key_env, model) = match kind {
            "jev" => (
                "Jev",
                JEV_ENDPOINT,
                "TYPESAFE_API_KEY",
                Some(model.unwrap_or(JEV_DEFAULT_MODEL).to_string()),
            ),
            "laya" => (
                "Laya",
                LAYA_ENDPOINT,
                "LAYA_API_KEY",
                model.map(String::from),
            ),
            other => bail!(
                "[recall] reranker = {other:?} is not supported: use \"jev\", \"jev:<model>\", \
                 \"laya\" or \"laya:<model>\", or \"\" to turn reranking off"
            ),
        };
        let key_env = match cfg.reranker_api_key_env.trim() {
            "" => default_key_env,
            name => name,
        };
        let api_key = std::env::var(key_env).ok().filter(|k| !k.is_empty());
        if name == "Jev" && api_key.is_none() {
            bail!("Set {key_env} to enable Jev reranking ([recall] reranker)");
        }
        let endpoint = match cfg.reranker_url.trim() {
            "" => default_url.to_string(),
            url => url.to_string(),
        };
        Ok(Some(Self {
            name,
            endpoint,
            model,
            api_key,
            timeout: Duration::from_millis(cfg.reranker_timeout_ms.max(1)),
        }))
    }

    /// Score `documents` against `query`: one score per document, in order.
    pub fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let mut request = ureq::post(&self.endpoint).timeout(self.timeout);
        if let Some(key) = &self.api_key {
            request = request.set("authorization", &format!("Bearer {key}"));
        }
        let response = request.send_json(build_request(self.model.as_deref(), query, documents));
        // Never echo the response body or key: the body may quote private memory text.
        let name = self.name;
        let body: Value = match response {
            Ok(resp) => resp.into_json()?,
            Err(ureq::Error::Status(code, _)) => bail!("{name} reranking failed (HTTP {code})"),
            Err(_) => bail!("{name} reranking request failed or timed out"),
        };
        scores(&body, documents.len())
    }
}

/// Build the batched `noul` request, preserving every candidate's full text.
/// No `model` key when `model` is `None`: a Laya server then routes the
/// request to the checkpoint that fits the text.
pub fn build_request(model: Option<&str>, query: &str, documents: &[&str]) -> Value {
    let questions: Map<String, Value> = documents
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            (
                format!("d{i}"),
                json!({
                    "type": "noul",
                    "instructions": format!("{INSTRUCTIONS}\nCandidate passage:\n{doc}"),
                    "criteria": { "true": CRITERIA_TRUE, "false": CRITERIA_FALSE },
                }),
            )
        })
        .collect();
    let mut request = json!({
        "state": { "query_excerpt": query },
        "questions": questions,
    });
    if let Some(model) = model {
        request["model"] = json!(model);
    }
    request
}

/// Validate exact answer coverage and finite probabilities in [0, 1].
pub fn scores(response: &Value, count: usize) -> Result<Vec<f32>> {
    let Some(answers) = response.get("answers").and_then(Value::as_object) else {
        bail!("reranker returned missing or unexpected answers");
    };
    if answers.len() != count || (0..count).any(|i| !answers.contains_key(&format!("d{i}"))) {
        bail!("reranker returned missing or unexpected answers");
    }
    (0..count)
        .map(|i| {
            let answer = &answers[&format!("d{i}")];
            if answer.get("type").and_then(Value::as_str) != Some("noul") {
                bail!("reranker returned an unexpected answer type");
            }
            // `as_f64` rejects bools and strings; JSON can't carry NaN/inf.
            let Some(score) = answer.get("noul").and_then(Value::as_f64) else {
                bail!("reranker returned a non-finite or non-numeric score");
            };
            if !(0.0..=1.0).contains(&score) {
                bail!("reranker score must be between zero and one");
            }
            Ok(score as f32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(scores: &[Value]) -> Value {
        let answers: Map<String, Value> = scores
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("d{i}"), json!({ "type": "noul", "noul": s })))
            .collect();
        json!({ "model": "jev-1.13.0", "answers": answers })
    }

    fn cfg(reranker: &str, url: &str, key_env: &str) -> RecallConfig {
        RecallConfig {
            reranker: reranker.into(),
            reranker_url: url.into(),
            reranker_api_key_env: key_env.into(),
            ..RecallConfig::default()
        }
    }

    #[test]
    fn request_preserves_full_content() {
        let long = "long memory ".repeat(2000);
        let req = build_request(Some("jev-latest"), "question", &[&long, "second"]);
        assert_eq!(req["model"], "jev-latest");
        assert_eq!(req["state"]["query_excerpt"], "question");
        assert_eq!(req["questions"].as_object().unwrap().len(), 2);
        assert!(
            req["questions"]["d0"]["instructions"]
                .as_str()
                .unwrap()
                .ends_with(&long)
        );
        let criteria = req["questions"]["d1"]["criteria"].as_object().unwrap();
        assert!(criteria.contains_key("true") && criteria.contains_key("false"));
    }

    #[test]
    fn request_without_model_lets_the_server_route() {
        let req = build_request(None, "q", &["doc"]);
        assert!(req.get("model").is_none(), "{req}");
    }

    #[test]
    fn valid_scores_parse_in_order() {
        let s = scores(&response(&[json!(0.2), json!(0.8), json!(1)]), 3).unwrap();
        assert_eq!(s, vec![0.2, 0.8, 1.0]);
    }

    /// A Laya answer carries extra keys (`confidence`, `answer_confidence`,
    /// `action`); the score is still its `noul` probability.
    #[test]
    fn laya_answer_extras_are_ignored() {
        let body = json!({ "answers": { "d0": {
            "type": "noul", "noul": 0.73, "confidence": 0.73,
            "answer_confidence": 0.73, "action": { "act_probability": 1.0 }
        }}});
        assert_eq!(scores(&body, 1).unwrap(), vec![0.73]);
    }

    #[test]
    fn invalid_scores_fail() {
        for bad in [
            Value::Null,
            json!(true),
            json!("0.5"),
            json!(-0.1),
            json!(1.1),
        ] {
            assert!(
                scores(&response(std::slice::from_ref(&bad)), 1).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn missing_extra_or_wrong_type_answers_fail() {
        assert!(scores(&response(&[]), 1).is_err());
        assert!(scores(&response(&[json!(0.1), json!(0.2)]), 1).is_err());
        let mut wrong = response(&[json!(0.5)]);
        wrong["answers"]["d0"]["type"] = json!("choice");
        assert!(scores(&wrong, 1).is_err());
    }

    #[test]
    fn empty_candidates_make_no_request() {
        let r = Reranker {
            name: "Laya",
            endpoint: "http://127.0.0.1:9/unused".into(),
            model: None,
            api_key: None,
            timeout: Duration::from_millis(1),
        };
        assert!(r.score("q", &[]).unwrap().is_empty());
    }

    #[test]
    fn config_selects_jev_or_laya() {
        assert_eq!(Reranker::from_config(&cfg("", "", "")).unwrap(), None);
        assert!(Reranker::from_config(&cfg("cross-encoder/foo", "", "")).is_err());

        // Laya: local default endpoint, no key needed, server-routed model.
        let laya = Reranker::from_config(&cfg("laya", "", "ICM_TEST_UNSET_KEY_VAR"))
            .unwrap()
            .unwrap();
        assert_eq!(laya.endpoint, LAYA_ENDPOINT);
        assert_eq!(laya.model, None);
        assert_eq!(laya.api_key, None);
        let pinned = Reranker::from_config(&cfg("laya:english", "http://h:1/v1/systemone", ""))
            .unwrap()
            .unwrap();
        assert_eq!(pinned.model.as_deref(), Some("english"));
        assert_eq!(pinned.endpoint, "http://h:1/v1/systemone");

        // Jev without its key is a configuration error, named.
        let err = Reranker::from_config(&cfg("jev", "", "ICM_TEST_UNSET_KEY_VAR"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("ICM_TEST_UNSET_KEY_VAR"), "{err}");
    }
}
