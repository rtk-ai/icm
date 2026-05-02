//! Output formats for `icm recall`.
//!
//! Three modes:
//! - `Toon` (default) — TOON one-row-per-memory, header declared once.
//!   Smallest token cost when stdout gets piped into an LLM context.
//! - `Detail` — the legacy multi-line labelled view, for humans reading
//!   the terminal (also the format expected by older scripts).
//! - `Json` — `serde_json` array, for tooling that wants to parse.

use anyhow::Result;
use clap::ValueEnum;
use icm_core::Memory;
use serde::Serialize;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RecallFormat {
    /// Compact tabular form (header + CSV rows). Default — best token cost.
    Toon,
    /// Legacy multi-line labelled output. Verbose; use for human reading.
    Detail,
    /// Machine-readable JSON array.
    Json,
}

/// Render a list of `(memory, score)` pairs into the chosen format.
///
/// `score` is the hybrid-search relevance score when available; the FTS
/// and keyword fallback paths in `cmd_recall` pass `None`. The TOON
/// header reflects whether any score is present so we don't emit a
/// useless empty column.
pub fn render(results: &[(Memory, Option<f32>)], format: RecallFormat) -> Result<String> {
    Ok(match format {
        RecallFormat::Toon => render_toon(results),
        RecallFormat::Detail => render_detail(results),
        RecallFormat::Json => render_json(results)?,
    })
}

fn render_toon(results: &[(Memory, Option<f32>)]) -> String {
    let has_score = results.iter().any(|(_, s)| s.is_some());
    let cols: &[&str] = if has_score {
        &["score", "id", "topic", "importance", "weight", "summary"]
    } else {
        &["id", "topic", "importance", "weight", "summary"]
    };

    let mut out = String::new();
    out.push_str(&format!(
        "memories[{}]{{{}}}:\n",
        results.len(),
        cols.join(",")
    ));
    for (m, score) in results {
        let weight = format!("{:.3}", m.weight);
        let importance = m.importance.to_string();
        let mut row: Vec<String> = Vec::with_capacity(cols.len());
        if has_score {
            row.push(
                score
                    .map(|s| format!("{s:.3}"))
                    .unwrap_or_else(|| "-".into()),
            );
        }
        row.push(m.id.clone());
        row.push(m.topic.clone());
        row.push(importance);
        row.push(weight);
        row.push(m.summary.clone());

        let escaped: Vec<String> = row.iter().map(|f| toon_escape(f)).collect();
        out.push_str("  ");
        out.push_str(&escaped.join(","));
        out.push('\n');
    }
    out
}

fn toon_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        let inner = field.replace('"', "\"\"");
        format!("\"{inner}\"")
    } else {
        field.to_string()
    }
}

fn render_detail(results: &[(Memory, Option<f32>)]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (m, score) in results {
        match score {
            Some(s) => writeln!(&mut out, "--- {} [score: {:.3}] ---", m.id, s).ok(),
            None => writeln!(&mut out, "--- {} ---", m.id).ok(),
        };
        let _ = writeln!(&mut out, "  topic:      {}", m.topic);
        let _ = writeln!(&mut out, "  importance: {}", m.importance);
        let _ = writeln!(&mut out, "  weight:     {:.3}", m.weight);
        let _ = writeln!(
            &mut out,
            "  created:    {}",
            m.created_at.format("%Y-%m-%d %H:%M")
        );
        let _ = writeln!(
            &mut out,
            "  accessed:   {} (x{})",
            m.last_accessed.format("%Y-%m-%d %H:%M"),
            m.access_count
        );
        let _ = writeln!(&mut out, "  summary:    {}", m.summary);
        if !m.keywords.is_empty() {
            let _ = writeln!(&mut out, "  keywords:   {}", m.keywords.join(", "));
        }
        if let Some(ref raw) = m.raw_excerpt {
            let _ = writeln!(&mut out, "  raw:        {raw}");
        }
        if score.is_none() && m.embedding.is_some() {
            let _ = writeln!(&mut out, "  embedding:  yes");
        }
        out.push('\n');
    }
    out
}

fn render_json(results: &[(Memory, Option<f32>)]) -> Result<String> {
    #[derive(Serialize)]
    struct Row<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        score: Option<f32>,
        #[serde(flatten)]
        memory: &'a Memory,
    }
    let rows: Vec<Row> = results
        .iter()
        .map(|(m, s)| Row {
            score: *s,
            memory: m,
        })
        .collect();
    Ok(serde_json::to_string_pretty(&rows)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use icm_core::Importance;

    fn fixture() -> Vec<(Memory, Option<f32>)> {
        let mut a = Memory::new("topic-a".into(), "first summary".into(), Importance::High);
        a.id = "01HZZ0".into();
        a.weight = 0.85;

        let mut b = Memory::new(
            "topic-b".into(),
            "second, with a comma".into(),
            Importance::Medium,
        );
        b.id = "01HZZ1".into();
        b.weight = 0.72;

        vec![(a, Some(0.91)), (b, Some(0.64))]
    }

    #[test]
    fn toon_includes_score_column_when_scored() {
        let s = render_toon(&fixture());
        let header = s.lines().next().unwrap();
        assert!(header.contains("score,"), "expected score column: {header}");
        assert_eq!(s.lines().count(), 3, "header + 2 rows");
    }

    #[test]
    fn toon_omits_score_column_when_none() {
        let mut data = fixture();
        for (_, s) in &mut data {
            *s = None;
        }
        let s = render_toon(&data);
        let header = s.lines().next().unwrap();
        assert!(
            !header.contains("score,"),
            "score column should be omitted: {header}"
        );
    }

    #[test]
    fn toon_escapes_commas_in_summary() {
        let s = render_toon(&fixture());
        assert!(
            s.contains("\"second, with a comma\""),
            "comma must be CSV-quoted in toon output:\n{s}"
        );
    }

    #[test]
    fn detail_renders_label_lines_per_memory() {
        let s = render_detail(&fixture());
        assert!(s.contains("topic:"));
        assert!(s.contains("importance:"));
        assert!(s.contains("--- 01HZZ0 [score: 0.910] ---"));
    }

    #[test]
    fn json_includes_score_field() {
        let s = render_json(&fixture()).unwrap();
        assert!(s.contains("\"score\""));
        assert!(s.contains("\"id\": \"01HZZ0\""));
    }

    #[test]
    fn empty_list_renders_clean() {
        let empty: Vec<(Memory, Option<f32>)> = Vec::new();
        assert_eq!(
            render_toon(&empty),
            "memories[0]{id,topic,importance,weight,summary}:\n"
        );
        assert_eq!(render_detail(&empty), "");
        assert_eq!(render_json(&empty).unwrap(), "[]");
    }
}
