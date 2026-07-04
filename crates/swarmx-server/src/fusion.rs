//! Answer/research fusion (OpenRouter-Fusion style), backed by the zulu panel.
//!
//! Given a question, run a PANEL of models in parallel (each a `zulu run` with a
//! different `-m` — one Comate license, N models), have a JUDGE model produce a
//! STRUCTURED comparison (consensus / contradictions / unique insights / blind
//! spots — a comparison, NOT a vote), then a SYNTHESIS model write the final
//! answer from that analysis. Distinct from the code-competition fusion in
//! `routes::workspaces`.
//!
//! Cost: N panel calls + 1 judge + 1 synthesis ≈ (N+2)× a single call — a
//! deliberate "high-value task consult", not a default path (P2.5 gates it).

use anyhow::{anyhow, Context, Result};
use futures::stream::{self, StreamExt};
use std::time::Instant;
use swarmx_protocol::rest::{
    FusionConsultRequest, FusionConsultResponse, FusionJudgeAnalysis, FusionPanelAnswer,
};

/// Cost guard: at most this many panel members (each is a real CLI call).
pub const MAX_PANEL: usize = 8;
/// Concurrency cap so a wide panel can't fire 8 comate.baidu.com calls at once.
const PANEL_CONCURRENCY: usize = 4;
/// A sensible cross-vendor default trio when the caller names no panel.
const DEFAULT_PANEL: &[&str] = &["Deepseek V4 Pro", "GLM-5.2", "Kimi-K2.6"];
/// Default judge/synthesis model — a strong all-rounder available under the
/// Comate license. (BC-4.5-S / Claude-via-Bedrock 403s AccessDenied on some
/// licenses, so default to DeepSeek V4 Pro, which is broadly available.)
const DEFAULT_JUDGE: &str = "Deepseek V4 Pro";

/// Run one `zulu run` Ask-mode query; return the answer text (frontmatter
/// stripped). Errors bubble up so the caller can mark that panelist failed.
async fn run_zulu_query(license: &str, model: &str, cwd: &str, prompt: &str) -> Result<String> {
    let out = tokio::process::Command::new("zulu")
        .args([
            "run", "-l", license, "-m", model, "-q", prompt, "--mode", "Ask", "--display", "task",
            "--cwd", cwd,
        ])
        .output()
        .await
        .context("spawn zulu run")?;
    if !out.status.success() {
        return Err(anyhow!(
            "zulu run [{model}] failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = strip_frontmatter(&String::from_utf8_lossy(&out.stdout));
    if text.trim().is_empty() {
        return Err(anyhow!("zulu run [{model}] produced no text"));
    }
    Ok(text)
}

/// Strip the `--- … ---` frontmatter block from `--display task` output → body.
fn strip_frontmatter(s: &str) -> String {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            // skip past the closing `---` line
            let after = &rest[end + 4..];
            let after = after.strip_prefix('\n').unwrap_or(after);
            return after.trim().to_string();
        }
    }
    t.trim().to_string()
}

/// Extract a JSON object from model output that may wrap it in ```json fences or
/// prose. Best-effort: first ```json block, else the first `{…}` span.
fn extract_json(s: &str) -> Option<serde_json::Value> {
    let try_parse = |txt: &str| serde_json::from_str::<serde_json::Value>(txt.trim()).ok();
    // fenced ```json … ```
    if let Some(i) = s.find("```json") {
        if let Some(end) = s[i + 7..].find("```") {
            if let Some(v) = try_parse(&s[i + 7..i + 7 + end]) {
                return Some(v);
            }
        }
    }
    if let Some(i) = s.find("```") {
        if let Some(end) = s[i + 3..].find("```") {
            if let Some(v) = try_parse(&s[i + 3..i + 3 + end]) {
                return Some(v);
            }
        }
    }
    // first balanced-ish {…}
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        return try_parse(&s[start..=end]);
    }
    None
}

fn str_list(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .filter(|s| !s.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The full panel → judge → synthesis pipeline.
pub async fn consult(
    req: &FusionConsultRequest,
    license: &str,
    cwd: &str,
) -> Result<FusionConsultResponse> {
    let question = req.question.trim();
    if question.is_empty() {
        return Err(anyhow!("question must not be empty"));
    }
    // Resolve + bound the panel.
    let mut panel: Vec<String> = req
        .panel
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if panel.is_empty() {
        panel = DEFAULT_PANEL.iter().map(|s| s.to_string()).collect();
    }
    panel.truncate(MAX_PANEL);

    // Panel: parallel (concurrency-capped) Ask-mode queries.
    let answers: Vec<FusionPanelAnswer> = stream::iter(panel.into_iter())
        .map(|m| async move {
            let t0 = Instant::now();
            let r = run_zulu_query(license, &m, cwd, question).await;
            let elapsed_ms = t0.elapsed().as_millis() as u64;
            match r {
                Ok(a) => FusionPanelAnswer { model: m, answer: a, ok: true, elapsed_ms },
                Err(e) => FusionPanelAnswer {
                    model: m,
                    answer: format!("(failed: {e})"),
                    ok: false,
                    elapsed_ms,
                },
            }
        })
        .buffer_unordered(PANEL_CONCURRENCY)
        .collect()
        .await;

    // Degradation: as long as ≥1 panelist answered, proceed (OpenRouter parity).
    if !answers.iter().any(|a| a.ok) {
        return Err(anyhow!("all {} panel members failed", answers.len()));
    }

    let judge_model = req.judge_model.clone().unwrap_or_else(|| DEFAULT_JUDGE.to_string());
    let analysis = judge(license, &judge_model, cwd, question, &answers).await;

    let synth_model = req.synthesis_model.clone().unwrap_or(judge_model);
    let synthesis = synthesize(license, &synth_model, cwd, question, &answers, &analysis)
        .await
        .unwrap_or_else(|e| format!("(synthesis failed: {e})"));

    let n = answers.len();
    Ok(FusionConsultResponse {
        question: question.to_string(),
        cost_note: format!("{n} panel + judge + synthesis ≈ {}× 单次调用", n + 2),
        panel: answers,
        analysis,
        synthesis,
    })
}

/// Judge stage: a structured JSON comparison of the panel answers.
async fn judge(
    license: &str,
    model: &str,
    cwd: &str,
    question: &str,
    answers: &[FusionPanelAnswer],
) -> FusionJudgeAnalysis {
    let mut body = String::new();
    for (i, a) in answers.iter().filter(|a| a.ok).enumerate() {
        body.push_str(&format!("\n### Answer {} (model: {})\n{}\n", i + 1, a.model, a.answer));
    }
    let prompt = format!(
        "You are an impartial JUDGE comparing multiple AI answers to one question. \
         Do NOT vote for a winner and do NOT merge the text. COMPARE them and return \
         ONLY a JSON object (no prose, no code fences) with these string arrays:\n\
         {{\"consensus\":[…points all or most answers agree on…],\
         \"contradictions\":[…where they disagree…],\
         \"unique_insights\":[…points only one answer raised…],\
         \"blind_spots\":[…important aspects none addressed…]}}\n\n\
         QUESTION:\n{question}\n\nANSWERS:{body}"
    );
    match run_zulu_query(license, model, cwd, &prompt).await {
        Ok(text) => match extract_json(&text) {
            Some(v) => FusionJudgeAnalysis {
                consensus: str_list(v.get("consensus")),
                contradictions: str_list(v.get("contradictions")),
                unique_insights: str_list(v.get("unique_insights")),
                blind_spots: str_list(v.get("blind_spots")),
                note: None,
            },
            None => FusionJudgeAnalysis {
                note: Some(format!("judge returned unstructured output: {}", truncate(&text, 500))),
                ..Default::default()
            },
        },
        Err(e) => FusionJudgeAnalysis {
            note: Some(format!("judge failed: {e}")),
            ..Default::default()
        },
    }
}

/// Synthesis stage: the outer model writes the final answer from the analysis.
async fn synthesize(
    license: &str,
    model: &str,
    cwd: &str,
    question: &str,
    answers: &[FusionPanelAnswer],
    analysis: &FusionJudgeAnalysis,
) -> Result<String> {
    let analysis_json = serde_json::to_string_pretty(analysis).unwrap_or_default();
    let mut body = String::new();
    for a in answers.iter().filter(|a| a.ok) {
        body.push_str(&format!("\n### {}\n{}\n", a.model, a.answer));
    }
    let prompt = format!(
        "You are the SYNTHESIZER. Using the panel answers and the judge's structured \
         analysis, write ONE clear, well-grounded final answer to the question. Lean on \
         the consensus, resolve the contradictions (say which side is better and why), \
         fold in the unique insights, and address the blind spots. Do not just \
         concatenate the answers.\n\n\
         QUESTION:\n{question}\n\nJUDGE ANALYSIS (JSON):\n{analysis_json}\n\nPANEL ANSWERS:{body}"
    );
    run_zulu_query(license, model, cwd, &prompt).await
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_block() {
        let s = "---\nconversationId: x\nstatus: ok\n---\nHello there.";
        assert_eq!(strip_frontmatter(s), "Hello there.");
        assert_eq!(strip_frontmatter("no frontmatter"), "no frontmatter");
    }

    #[test]
    fn extract_json_from_fences_and_prose() {
        let fenced = "Sure:\n```json\n{\"consensus\":[\"a\"]}\n```\n";
        let v = extract_json(fenced).unwrap();
        assert_eq!(v["consensus"][0], "a");
        let prose = "Here { \"blind_spots\": [\"x\",\"y\"] } done";
        let v2 = extract_json(prose).unwrap();
        assert_eq!(str_list(v2.get("blind_spots")), vec!["x", "y"]);
        assert!(extract_json("no json here").is_none());
    }

    #[test]
    fn str_list_filters_empty_and_nonstrings() {
        let v = serde_json::json!({"k": ["a", "", "  ", 3, "b"]});
        assert_eq!(str_list(v.get("k")), vec!["a", "b"]);
        assert!(str_list(None).is_empty());
    }
}
