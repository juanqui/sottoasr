//! Canonical vocabulary validation and conservative acoustic candidate selection.

use std::collections::HashSet;

pub const MAX_TERMS: usize = 100;
pub const MAX_TERM_CHARS: usize = 120;
pub const DOWNLOAD_SIZE_MB: u64 = 103;

pub fn validate(terms: &[String]) -> Result<(), String> {
    if terms.len() > MAX_TERMS {
        return Err(format!("Vocabulary supports at most {MAX_TERMS} terms"));
    }
    let mut seen = HashSet::new();
    for term in terms {
        if term.trim().is_empty() || term.chars().count() > MAX_TERM_CHARS {
            return Err(format!("Vocabulary terms must contain 1–{MAX_TERM_CHARS} characters"));
        }
        if term.chars().any(char::is_control) {
            return Err("Vocabulary terms cannot contain control characters".into());
        }
        if !seen.insert(term.trim().to_lowercase()) {
            return Err("Vocabulary terms must be unique (ignoring case)".into());
        }
    }
    Ok(())
}

/// Raw acoustic comparison. The SDK's additive boost is deliberately omitted.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Candidate {
    pub term: String,
    pub start: usize,
    pub end: usize,
    pub similarity: f64,
    pub vocabulary_score: f64,
    pub original_score: f64,
}

/// Frozen candidate policy, 2026-09-08. It can decline ambiguous words.
/// It never reconstitutes the transcript from the SDK's normalized word list.
pub fn apply_candidates(base: &str, terms: &[String], candidates: &[Candidate]) -> String {
    let mut accepted: Vec<&Candidate> = candidates.iter().filter(|candidate| {
        let Some(span) = base.get(candidate.start..candidate.end) else { return false; };
        if span.is_empty() || !terms.contains(&candidate.term)
            || !candidate.similarity.is_finite()
            || !candidate.vocabulary_score.is_finite()
            || !candidate.original_score.is_finite()
            || candidate.vocabulary_score <= candidate.original_score
            || !span.chars().all(|character| character.is_alphabetic() || character == ' ')
            || !candidate.term.chars().all(|character| character.is_alphabetic() || character == ' ')
        {
            return false;
        }
        let term_letters = candidate.term.chars().filter(|character| character.is_alphabetic()).count();
        let base_letters = span.chars().filter(|character| character.is_alphabetic()).count();
        if term_letters <= 5 {
            term_letters >= 3 && base_letters == term_letters && candidate.similarity >= 0.75
        } else {
            candidate.similarity >= 0.8
        }
    }).collect();
    accepted.sort_by(|left, right| (left.start, left.end, &left.term).cmp(&(right.start, right.end, &right.term)));
    accepted.dedup_by(|right, left| right.start == left.start && right.end == left.end && right.term == left.term);
    let mut result = base.to_owned();
    for (index, candidate) in accepted.iter().enumerate().rev() {
        // Conflicting proposals are declined together instead of selecting an
        // arbitrary vocabulary entry based on the order of the user's list.
        if accepted.iter().enumerate().any(|(other_index, other)| {
            index != other_index && candidate.start < other.end && other.start < candidate.end
        }) {
            continue;
        }
        result.replace_range(candidate.start..candidate.end, &candidate.term);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_unicode_limits_duplicates_and_controls() {
        assert!(validate(&["Qwen3.8-Flash-Next".into(), "日本語".into()]).is_ok());
        assert!(validate(&["é".repeat(120)]).is_ok());
        assert!(validate(&["é".repeat(121)]).is_err());
        assert!(validate(&["Qwen".into(), " qWEN ".into()]).is_err());
        assert!(validate(&["Qwen\n".into()]).is_err());
        assert!(validate(&[" ".into()]).is_err());
        assert!(validate(&vec!["term".into(); 101]).is_err());
    }

    #[test]
    fn settings_normalize_canonical_terms_and_preserve_replacements() {
        let mut settings = crate::models::Settings {
            vocabulary: vec!["  Qwen  ".into(), "CoreML".into()],
            ..Default::default()
        };
        settings.normalize().unwrap();
        assert_eq!(settings.vocabulary, ["Qwen", "CoreML"]);
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy.as_object_mut().unwrap().remove("vocabulary");
        let restored: crate::models::Settings = serde_json::from_value(legacy).unwrap();
        assert!(restored.vocabulary.is_empty());
        assert_ne!(settings, restored);
    }
}
