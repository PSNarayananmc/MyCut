//! Agent flow: context → provider → parse → validate → repair loop (≤2
//! corrections) → apply as one transaction. Never executes unvalidated plans.

use std::sync::atomic::AtomicBool;

use mycut_core::{History, Project};
use mycut_schema::apply::plan_to_commands;
use mycut_schema::validate::PlanContext;
use mycut_schema::EditPlan;

use crate::prompt::{build_messages, system_prompt, ContextSummary};
use crate::provider::{AIProvider, AiError, ChatMessage, PlanRequest};

/// Max correction attempts after the initial request (spec: at most 2).
pub const MAX_REPAIRS: u32 = 2;

#[derive(Debug, Default)]
pub struct AgentOutcome {
    pub applied: bool,
    pub summary: String,
    pub clarification: Option<String>,
    pub assumptions: Vec<String>,
    pub warnings: Vec<String>,
    pub repairs: Vec<String>,
    pub error: Option<AiError>,
    pub requests_made: u32,
}

/// Run the full plan flow.
pub fn plan_and_apply(
    provider: &dyn AIProvider,
    project: &mut Project,
    history: &mut History,
    schema_ctx: &PlanContext,
    summary: &ContextSummary,
    frames_enabled: bool,
    cancel: &AtomicBool,
) -> AgentOutcome {
    let caps = provider.capabilities();
    let messages = build_messages(summary, frames_enabled, caps.vision);
    let schema: serde_json::Value =
        serde_json::from_str(mycut_schema::EDIT_PLAN_SCHEMA_JSON).unwrap_or(serde_json::json!({}));
    let mut convo: Vec<ChatMessage> = vec![ChatMessage {
        role: "system".into(),
        content: system_prompt(),
        image_url: None,
    }];
    convo.extend(messages);

    let req = PlanRequest {
        system_prompt: system_prompt(),
        messages: convo.clone(),
        json_schema: if caps.structured_output {
            Some(schema)
        } else {
            None
        },
        max_tokens: 4096,
        temperature: 0.2,
    };

    let mut outcome = AgentOutcome::default();
    let mut attempt = 0u32;
    let mut current_messages = req.messages.clone();

    loop {
        outcome.requests_made += 1;
        let Ok(resp) = provider.complete(
            &PlanRequest {
                system_prompt: system_prompt(),
                messages: current_messages.clone(),
                json_schema: req.json_schema.clone(),
                max_tokens: req.max_tokens,
                temperature: req.temperature,
            },
            cancel,
        ) else {
            outcome.error = Some(match provider.complete(&req, cancel) {
                Ok(_) => AiError::Provider("inconsistent provider state".into()),
                Err(e) => e,
            });
            return outcome;
        };

        // Trivially-safe repair: strip markdown fences if present.
        let text = strip_fences(&resp.text);
        match EditPlan::parse(&text) {
            Ok(plan) => {
                // Apply through the validator (no unvalidated execution).
                match plan_to_commands(&plan, project, history, schema_ctx) {
                    Ok(app) => {
                        outcome.applied = true;
                        outcome.summary = plan.intent_summary.clone();
                        outcome.clarification = plan.needs_clarification.clone();
                        outcome.assumptions = plan.assumptions.clone();
                        outcome.warnings = app.warnings;
                        outcome.repairs = app.repairs;
                        if outcome.summary.is_empty() {
                            outcome.summary = "Applied the edit plan".into();
                        }
                        return outcome;
                    }
                    Err(plan_err) => {
                        if attempt >= MAX_REPAIRS {
                            outcome.error = Some(AiError::BadOutput(plan_err.to_string()));
                            return outcome;
                        }
                        attempt += 1;
                        current_messages.push(ChatMessage {
                            role: "assistant".into(),
                            content: resp.text.clone(),
                            image_url: None,
                        });
                        current_messages.push(ChatMessage {
                            role: "user".into(),
                            content: format!(
                                "Your plan was rejected by the local validator:\n{plan_err}\nReturn a corrected plan JSON document only. Fix the reported issues without changing the intent."
                            ),
                            image_url: None,
                        });
                    }
                }
            }
            Err(parse_err) => {
                if attempt >= MAX_REPAIRS {
                    outcome.error = Some(AiError::BadOutput(parse_err.to_string()));
                    return outcome;
                }
                attempt += 1;
                current_messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: resp.text.clone(),
                    image_url: None,
                });
                current_messages.push(ChatMessage {
                    role: "user".into(),
                    content: format!(
                        "Your output was not a valid Edit Plan JSON: {parse_err}\nReturn ONLY the corrected JSON document matching the MyCut Edit Plan schema 1.0."
                    ),
                    image_url: None,
                });
            }
        }
    }
}

/// Strip ``` fences and leading prose before the first `{`.
#[must_use]
pub fn strip_fences(text: &str) -> String {
    let t = text.trim();
    let t = if let Some(rest) = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")) {
        rest.trim_start()
    } else {
        t
    };
    let t = if let Some(pos) = t.rfind("```") {
        &t[..pos]
    } else {
        t
    };
    // Drop any prose before the first '{'.
    match t.find('{') {
        Some(pos) => t[pos..].trim().to_string(),
        None => t.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_stripped() {
        assert_eq!(strip_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fences("Here is the plan:\n{\"a\":1}"), "{\"a\":1}");
        assert_eq!(strip_fences("{\"a\":1}"), "{\"a\":1}");
    }
}
