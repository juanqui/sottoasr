//! JSON-lines adapter for benchmarking the production Rust edit boundary.
#[path = "../src/llm/edits.rs"]
mod edits;

use std::io::{self, BufRead};

fn main() {
    for line in io::stdin().lock().lines() {
        let result = line.map_err(|e| e.to_string()).and_then(|line| {
            let value: serde_json::Value =
                serde_json::from_str(&line).map_err(|e| e.to_string())?;
            let text = value
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("Missing text")?;
            let candidates = edits::deletion_candidates(text);
            if let Some(id) = value.get("context_id").and_then(|v| v.as_u64()) {
                let (before, span, after) = candidates
                    .get(id as usize)
                    .and_then(|candidate| candidate.source_context(text))
                    .ok_or("Invalid candidate context ID")?;
                Ok(serde_json::json!({"before": before, "span": span, "after": after}))
            } else if let Some(ids) = value.get("delete_ids") {
                let ids: Vec<usize> =
                    serde_json::from_value(ids.clone()).map_err(|e| e.to_string())?;
                edits::apply_deletions(text, &candidates, &ids)
                    .map(|text| serde_json::json!({"text": text}))
            } else {
                Ok(serde_json::json!({"candidates": candidates}))
            }
        });
        println!(
            "{}",
            result.unwrap_or_else(|error| serde_json::json!({"error": error}))
        );
    }
}
