//! Release smoke adapter: import the production validator, never a Python copy.
#[path = "../../../../../src-tauri/src/llm/validation.rs"]
mod validation;

use std::io::{self, BufRead};

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("read JSON line");
        let request: serde_json::Value = serde_json::from_str(&line).expect("valid JSON request");
        let source = request["source"].as_str().expect("source text");
        let language_skip = whatlang::detect(source)
            .is_some_and(|info| info.is_reliable() && info.lang() != whatlang::Lang::Eng);
        let response = if request["action"] == "admit" {
            serde_json::json!({"language_skip":language_skip})
        } else {
            let proposal = request["proposal"].as_str().expect("proposal text");
            let terms: Vec<String> = request["protected_terms"].as_array()
                .map(|items| items.iter().map(|item| item.as_str().expect("term text").to_string()).collect())
                .unwrap_or_default();
            if language_skip {
                serde_json::json!({"accepted":false,"language_skip":true,"output":source,"reason":"reliable_non_english"})
            } else {
                match validation::validate_cleanup(source, proposal, &terms) {
                    Ok(output) => serde_json::json!({"accepted":true,"language_skip":false,"output":output}),
                    Err(reason) => serde_json::json!({"accepted":false,"language_skip":false,"output":source,"reason":reason}),
                }
            }
        };
        println!("{response}");
    }
}
