//! Enumerate narrow deletion choices and apply them to the original transcript.
//! Model-generated text never reaches the clipboard: the model only selects IDs.

use std::collections::HashSet;

use serde::Serialize;

/// A proposed deletion. Byte ranges stay in Rust and are never model-controlled.
#[derive(Clone, Debug, Serialize)]
pub struct DeletionCandidate {
    pub id: usize,
    pub text: String,
    pub kind: &'static str,
    #[serde(skip)]
    start: usize,
    #[serde(skip)]
    end: usize,
}

impl DeletionCandidate {
    /// Expose this exact occurrence for classifier experiments. The model still
    /// cannot supply or alter these bounds; production application uses IDs.
    #[allow(dead_code)] // Used by the standalone benchmark adapter.
    pub fn source_context<'a>(&self, original: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
        Some((
            original.get(..self.start)?,
            original.get(self.start..self.end)?,
            original.get(self.end..)?,
        ))
    }
}

/// Avoid unbounded prompts on very long dictations. Such inputs pass through.
pub const MAX_CLEANUP_CHARS: usize = 16_000;
const MAX_CANDIDATES: usize = 128;

fn quoted_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut open: Option<(char, usize)> = None;
    for (pos, ch) in text.char_indices() {
        // Apostrophes within contractions are content, including inside a
        // single-quoted span. Escaped delimiters likewise do not end a quote.
        let inside_word = matches!(ch, '\'' | '’')
            && text[..pos].ends_with(char::is_alphanumeric)
            && text[pos + ch.len_utf8()..].starts_with(char::is_alphanumeric);
        let escaped = text[..pos]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            % 2
            == 1;
        if inside_word || escaped {
            continue;
        }
        if let Some((close, start)) = open {
            if ch == close {
                ranges.push((start, pos + ch.len_utf8()));
                open = None;
            }
        } else {
            let close = match ch {
                '"' | '`' => Some(ch),
                '“' => Some('”'),
                '‘' => Some('’'),
                '\'' if !text[..pos].ends_with(char::is_alphanumeric) => Some('\''),
                _ => None,
            };
            if let Some(close) = close {
                open = Some((close, pos));
            }
        }
    }
    if let Some((_, start)) = open {
        ranges.push((start, text.len()));
    }
    ranges
}

/// Only set-off hesitation words and adjacent function-word stutters qualify.
/// Ambiguous crutches (like/right/so), names, numbers, negations, content-word
/// emphasis, quoted text, and code are deliberately outside this edit vocabulary.
pub fn deletion_candidates(text: &str) -> Vec<DeletionCandidate> {
    if text.chars().count() > MAX_CLEANUP_CHARS {
        return Vec::new();
    }
    let quotes = quoted_ranges(text);
    let tokens: Vec<(usize, &str)> = text
        .split_whitespace()
        .scan(0, |offset, token| {
            let pos = *offset + text[*offset..].find(token)?;
            *offset = pos + token.len();
            Some((pos, token))
        })
        .collect();
    let mut result = Vec::new();
    for (i, &(start, token)) in tokens.iter().enumerate() {
        if quotes
            .iter()
            .any(|&(a, b)| start < b && start + token.len() > a)
        {
            continue;
        }
        let word = token.strip_suffix(',').unwrap_or(token);
        // Require a comma separating a hesitation from the following speech.
        // This excludes names/mentions such as "Um is a surname" and identifiers.
        let hesitation = token.ends_with(',') && matches!(word, "um" | "uh" | "uhm" | "erm");
        let repeat = matches!(
            token,
            "I" | "i" | "the" | "a" | "an" | "to" | "we" | "it" | "and" | "of"
        ) && tokens.get(i + 1).is_some_and(|&(next_start, next)| {
            next == token
                && text[start + token.len()..next_start]
                    .chars()
                    .all(|ch| matches!(ch, ' ' | '\t'))
        });
        if !hesitation && !repeat {
            continue;
        }
        let token_end = start + token.len();
        let end = token_end
            + text[token_end..]
                .chars()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .map(char::len_utf8)
                .sum::<usize>();
        // Never remove the last remaining word, even if the model requests it.
        if end == text.len() {
            continue;
        }
        result.push(DeletionCandidate {
            id: result.len(),
            text: word.to_string(),
            kind: if hesitation {
                "hesitation"
            } else {
                "repeated word"
            },
            start,
            end,
        });
        if result.len() > MAX_CANDIDATES {
            return Vec::new();
        }
    }
    result
}

/// Apply only IDs from the Rust-generated set, copying every other byte.
pub fn apply_deletions(
    original: &str,
    candidates: &[DeletionCandidate],
    selected: &[usize],
) -> Result<String, String> {
    let mut seen = HashSet::new();
    for &id in selected {
        if id >= candidates.len() || !seen.insert(id) {
            return Err("Cleanup returned invalid or duplicate deletion IDs".into());
        }
    }
    let mut out = String::with_capacity(original.len());
    let mut cursor = 0;
    for candidate in candidates.iter().filter(|c| seen.contains(&c.id)) {
        out.push_str(&original[cursor..candidate.start]);
        cursor = candidate.end;
    }
    out.push_str(&original[cursor..]);
    if out.trim().is_empty() {
        return Err("Cleanup would remove the entire transcript".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_only_selected_spans_and_preserves_tail() {
        let raw = "um, I I need the the report. Code: 8472, total: $51.75.";
        let edits = deletion_candidates(raw);
        assert_eq!(edits.len(), 3);
        assert_eq!(
            apply_deletions(raw, &edits, &[2, 0, 1]).unwrap(),
            "I need the report. Code: 8472, total: $51.75."
        );
    }

    #[test]
    fn candidate_context_marks_the_exact_unicode_source_occurrence() {
        let raw = "Mañana um, I I will say um, again.";
        let candidates = deletion_candidates(raw);
        assert_eq!(
            candidates[2].source_context(raw),
            Some(("Mañana um, I I will say ", "um, ", "again."))
        );
        let (before, span, after) = candidates[1].source_context(raw).unwrap();
        assert_eq!(format!("{before}{span}{after}"), raw);
        assert_eq!(
            format!("{before}{after}"),
            apply_deletions(raw, &candidates, &[1]).unwrap()
        );
        assert!(candidates[2].source_context("short").is_none());
    }

    #[test]
    fn preserves_ambiguous_words_names_negations_and_emphasis() {
        for raw in [
            "Um is a Korean surname.",
            "Um, Kim, and Lee joined the meeting.",
            "I like this, right?",
            "No no never never do that.",
            "Very very important. 10 10 milligrams.",
            "Use Qwen3.8-Flash-Next at http://localhost:8080/v1.",
        ] {
            assert!(deletion_candidates(raw).is_empty(), "{raw}");
        }
    }

    #[test]
    fn quoted_and_code_tokens_are_protected() {
        for raw in [
            "Say \"um, I I\" exactly.",
            "Write `um, I I` in code.",
            "Say ‘um, the the’ again.",
            "Say 'um, the the' exactly.",
            "Say 'I I don't use um, fillers' exactly.",
            "Say ‘I I don’t use um, fillers’ exactly.",
            r#"Say "I don't say \"um,\" or uh, here" exactly."#,
            "Unclosed \"um, I I",
        ] {
            assert!(deletion_candidates(raw).is_empty(), "{raw}");
        }
    }

    #[test]
    fn unicode_and_original_spacing_survive() {
        let raw = "um, necesito probar Qwen mañana.\n  La fecha es el 12.";
        assert_eq!(
            apply_deletions(raw, &deletion_candidates(raw), &[0]).unwrap(),
            "necesito probar Qwen mañana.\n  La fecha es el 12."
        );
    }

    #[test]
    fn triple_stutter_keeps_one_and_invalid_ids_fail_closed() {
        let raw = "I I I need to leave now.";
        let edits = deletion_candidates(raw);
        assert_eq!(
            apply_deletions(raw, &edits, &[0, 1]).unwrap(),
            "I need to leave now."
        );
        assert!(apply_deletions(raw, &edits, &[2]).is_err());
        assert!(apply_deletions(raw, &edits, &[0, 0]).is_err());
        assert_eq!(apply_deletions(raw, &edits, &[]).unwrap(), raw);
    }

    #[test]
    fn paragraph_breaks_are_not_stutters_or_deleted() {
        assert!(deletion_candidates("I\n\nI need this paragraph.").is_empty());
        let raw = "um,\n\nI need this paragraph.";
        assert_eq!(
            apply_deletions(raw, &deletion_candidates(raw), &[0]).unwrap(),
            "\n\nI need this paragraph."
        );
    }

    #[test]
    fn final_word_and_oversized_inputs_pass_through() {
        assert!(deletion_candidates("um,").is_empty());
        assert!(deletion_candidates(&"um, ".repeat(MAX_CLEANUP_CHARS)).is_empty());
    }
}
