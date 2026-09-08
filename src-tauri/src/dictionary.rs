//! Explicit local spelling corrections. No fuzzy matching or inferred aliases.

use std::collections::HashSet;
use std::ops::Range;

use crate::models::DictionaryEntry;

const MAX_ENTRIES: usize = 200;
const MAX_FIELD_CHARS: usize = 120;

fn valid_field(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= MAX_FIELD_CHARS
        && !value.chars().any(char::is_control)
        && value.chars().any(char::is_alphanumeric)
}

pub fn validate(entries: &[DictionaryEntry]) -> Result<(), String> {
    if entries.len() > MAX_ENTRIES {
        return Err(format!("Dictionary supports up to {MAX_ENTRIES} entries"));
    }
    let mut heard = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        for (label, value) in [
            ("Heard", &entry.heard),
            ("Write instead", &entry.replacement),
        ] {
            if !valid_field(value) {
                return Err(format!(
                    "Dictionary row {}: {label} must contain a word, use at most {MAX_FIELD_CHARS} characters, and have no surrounding whitespace or control characters",
                    index + 1,
                ));
            }
        }
        if !heard.insert(entry.heard.to_ascii_lowercase()) {
            return Err(format!(
                "Dictionary row {} duplicates a Heard alias",
                index + 1
            ));
        }
    }
    Ok(())
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn is_connector(character: char) -> bool {
    matches!(character, '.' | '/' | '\\' | '-' | '+')
}

fn has_boundaries(text: &str, start: usize, end: usize) -> bool {
    let mut before = text[..start].chars().rev();
    if let Some(previous) = before.next() {
        if is_word(previous)
            || matches!(previous, '/' | '\\')
            || (is_connector(previous) && before.next().is_some_and(is_word))
        {
            return false;
        }
    }
    let mut after = text[end..].chars();
    if let Some(next) = after.next() {
        if is_word(next)
            || matches!(next, '/' | '\\')
            || (is_connector(next) && after.next().is_some_and(is_word))
        {
            return false;
        }
    }
    true
}

/// Backtick code and obvious URL/email tokens are opaque to the dictionary.
fn protected_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(relative) = text[offset..].find('`') {
        let start = offset + relative;
        let ticks = text[start..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        let delimiter = &text[start..start + ticks];
        let content_start = start + ticks;
        let end = text[content_start..]
            .find(delimiter)
            .map_or(text.len(), |close| content_start + close + ticks);
        ranges.push(start..end);
        offset = end;
    }

    offset = 0;
    for token in text.split_whitespace() {
        let start = offset + text[offset..].find(token).expect("token belongs to input");
        let end = start + token.len();
        let token_start = token.trim_start_matches(|character: char| !character.is_alphanumeric());
        if token.contains("://")
            || token.contains('@')
            || token_start.to_ascii_lowercase().starts_with("www.")
        {
            ranges.push(start..end);
        }
        offset = end;
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

/// Replace explicit aliases once, longest first, without changing untouched spans.
/// ASCII case is ignored; Unicode bytes and literal phrase spacing are preserved.
pub fn apply(text: &str, entries: &[DictionaryEntry]) -> String {
    if entries.is_empty() || text.is_empty() {
        return text.to_owned();
    }
    // Settings writes validate the entire list. Bound work defensively when an
    // older/manually edited settings file is loaded without that validation.
    let mut aliases: Vec<_> = entries
        .iter()
        .take(MAX_ENTRIES)
        .filter(|entry| valid_field(&entry.heard) && valid_field(&entry.replacement))
        .collect();
    aliases.sort_by_key(|entry| std::cmp::Reverse(entry.heard.len()));
    let protected = protected_ranges(text);
    let mut protected_index = 0;
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;

    while offset < text.len() {
        while protected
            .get(protected_index)
            .is_some_and(|range| range.end <= offset)
        {
            protected_index += 1;
        }
        let next_protected = protected.get(protected_index);
        if let Some(range) = next_protected.filter(|range| range.start <= offset) {
            output.push_str(&text[offset..range.end]);
            offset = range.end;
            continue;
        }
        let matched = aliases.iter().find(|entry| {
            let end = offset + entry.heard.len();
            next_protected.map_or(true, |range| end <= range.start)
                && text
                    .get(offset..end)
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&entry.heard))
                && has_boundaries(text, offset, end)
        });
        if let Some(entry) = matched {
            output.push_str(&entry.replacement);
            offset += entry.heard.len();
        } else {
            let character = text[offset..]
                .chars()
                .next()
                .expect("offset is within input");
            output.push(character);
            offset += character.len_utf8();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(heard: &str, replacement: &str) -> DictionaryEntry {
        DictionaryEntry {
            heard: heard.into(),
            replacement: replacement.into(),
        }
    }

    #[test]
    fn matches_case_and_preserves_punctuation_and_whitespace() {
        assert_eq!(
            apply("Use QUEN, then quen.\n\tQuen!", &[entry("Quen", "Qwen")]),
            "Use Qwen, then Qwen.\n\tQwen!"
        );
    }

    #[test]
    fn rejects_partial_words_unicode_words_and_identifiers() {
        let input = "sequence Quenya éQuen Quené my_quen quen.com /quen quen/model Quen3.8-Flash-Next my-quen quen-based";
        assert_eq!(apply(input, &[entry("Quen", "Qwen")]), input);
        assert_eq!(
            apply(
                "Quen3.8-Flash-Next",
                &[entry("Quen3.8-Flash-Next", "Qwen3.8-Flash-Next")]
            ),
            "Qwen3.8-Flash-Next"
        );
    }

    #[test]
    fn longest_phrase_wins_and_replacements_do_not_cascade() {
        let entries = [
            entry("Quen", "Qwen"),
            entry("Quen next", "Qwen Next"),
            entry("Qwen", "Other"),
        ];
        assert_eq!(apply("Quen next, Quen", &entries), "Qwen Next, Qwen");
        assert_eq!(
            apply("Quen  next", &[entry("Quen next", "Qwen Next")]),
            "Quen  next"
        );
    }

    #[test]
    fn leaves_code_urls_and_email_unchanged() {
        let input = "`Quen` ```Quen``` https://example.com/Quen (www.example.com/Quen) Quen@example.com Quen `unclosed Quen";
        assert_eq!(apply(input, &[entry("Quen", "Qwen")]),
            "`Quen` ```Quen``` https://example.com/Quen (www.example.com/Quen) Quen@example.com Qwen `unclosed Quen");
    }

    #[test]
    fn can_match_unicode_alias_without_normalizing_or_changing_case() {
        assert_eq!(
            apply("café CAFÉ cafe", &[entry("café", "Café")]),
            "Café CAFÉ cafe"
        );
    }

    #[test]
    fn validates_aliases_and_bounds() {
        assert!(validate(&[]).is_ok());
        assert!(validate(&[entry("Quen", "Qwen")]).is_ok());
        for value in ["", " Quen", "Quen ", "Quen\nNext", "!!!"] {
            assert!(validate(&[entry(value, "Qwen")]).is_err());
            assert!(validate(&[entry("Quen", value)]).is_err());
        }
        assert!(validate(&[entry("Quen", "Qwen"), entry("QUEN", "Other")]).is_err());
        assert!(validate(&[entry(&"x".repeat(121), "Qwen")]).is_err());
        assert!(validate(&vec![entry("Quen", "Qwen"); 201]).is_err());
    }

    #[test]
    fn empty_or_invalid_dictionary_cannot_erase_text() {
        let input = "Keep every word.";
        assert_eq!(apply(input, &[]), input);
        assert_eq!(apply(input, &[entry("Keep", "")]), input);
    }
}
