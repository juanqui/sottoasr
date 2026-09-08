//! Source-preserving validation of a completed cleanup proposal.
//!
//! Policy port of frozen experimental v6 (SHA256 4a8bc2a49feb6b944e765ec238f7bdc381a65576907d1b8ddddf9a4b172b177b).
//! The model chooses edits; this module permits only bounded source deletions
//! and reconstructs the delivered text from the source. Rejection must retain
//! the original transcript. Syntactic guards do not establish semantic safety.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::OnceLock;

use focaccia::unicode_full_case_eq;
use regex::Regex;

pub const MAX_CLEANUP_BYTES: usize = 32_000;
pub const MAX_CLEANUP_WORDS: usize = 1_024;
const MAX_ALIGNMENT_STATES: usize = 20_000;
const MAX_ALIGNMENTS: usize = 128;

macro_rules! re {
    ($pattern:literal) => {{
        static PATTERN: OnceLock<Regex> = OnceLock::new();
        PATTERN.get_or_init(|| Regex::new($pattern).expect("valid cleanup pattern"))
    }};
}

const HESITATIONS: &[&str] = &["um", "uh", "erm", "er", "uhm", "umm", "hmm"];
const ARTICLES: &[&str] = &["a", "an", "the"];
const DETERMINERS: &[&str] = &["a", "an", "the", "this", "that", "these", "those"];
const NEGATIONS: &[&str] = &["no", "not", "never", "neither", "nor", "without"];
const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "i", "we", "you", "he", "she", "it", "they", "to", "of", "for", "from", "in",
    "on", "at", "with", "by", "and", "or", "is", "are", "was", "were", "have", "has", "had",
    "this", "that",
];
// This exclusion applies only to isolated repeated words, not phrase restarts.
const SINGLE_REPEAT_PROTECTED: &[&str] = &[
    "very",
    "really",
    "so",
    "too",
    "much",
    "more",
    "less",
    "quite",
    "rather",
    "extremely",
    "absolutely",
    "yes",
    "yeah",
    "yep",
    "yup",
    "nope",
    "okay",
    "ok",
    "right",
    "hey",
    "wait",
    "stop",
    "please",
];
const HONORIFICS: &[&str] = &["dr", "mr", "mrs", "ms", "mx", "prof", "rev", "hon"];
const LITERAL_NOUNS: &[&str] = &[
    "string",
    "literal",
    "label",
    "labels",
    "tag",
    "tags",
    "parameter",
    "parameters",
];
const MENTION_MARKERS: &[&str] = &[
    "word",
    "words",
    "token",
    "tokens",
    "name",
    "names",
    "letter",
    "letters",
    "identifier",
    "identifiers",
    "sequence",
    "spell",
    "type",
];
const UNITS: &[&str] = &[
    "mm", "cm", "m", "km", "kg", "g", "mg", "lb", "lbs", "ms", "s", "seconds", "minutes", "hours",
    "inches", "feet", "percent", "dollars", "volts", "amps", "watts",
];

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    value: &'a str,
    start: usize,
    end: usize,
}

impl Token<'_> {
    fn is(&self, word: &str) -> bool {
        unicode_full_case_eq(self.value, word)
    }

    fn in_set(&self, words: &[&str]) -> bool {
        words.iter().any(|word| self.is(word))
    }
}

fn word_pattern() -> &'static Regex {
    // Python's Unicode16 \w is Letters/Numbers/underscore. Rust's \w also
    // includes marks, connector punctuation and join controls, so spell it out.
    re!(r"[\p{L}\p{N}_]+(?:['’][\p{L}\p{N}_]+)*")
}

fn tokenize(text: &str) -> Vec<Token<'_>> {
    word_pattern()
        .find_iter(text)
        .map(|m| Token {
            value: m.as_str(),
            start: m.start(),
            end: m.end(),
        })
        .collect()
}

fn word_char(c: char) -> bool {
    re!(r"\A[\p{L}\p{N}_]\z").is_match(c.encode_utf8(&mut [0; 4]))
}

fn upper(c: char) -> bool {
    re!(r"\A\p{Uppercase}\z").is_match(c.encode_utf8(&mut [0; 4]))
}

fn python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn python_digit(c: char) -> bool {
    // Unicode16 Numeric_Type=Digit outside Decimal_Number, from the frozen
    // Python3.14 oracle. Fractions and other numeric letters are not digits.
    re!(r"\A[\p{Nd}\u{b2}-\u{b3}\u{b9}\u{1369}-\u{1371}\u{19da}\u{2070}\u{2074}-\u{2079}\u{2080}-\u{2089}\u{2460}-\u{2468}\u{2474}-\u{247c}\u{2488}-\u{2490}\u{24ea}\u{24f5}-\u{24fd}\u{24ff}\u{2776}-\u{277e}\u{2780}-\u{2788}\u{278a}-\u{2792}\u{10a40}-\u{10a43}\u{10e60}-\u{10e68}\u{11052}-\u{1105a}\u{1f100}-\u{1f10a}]\z")
        .is_match(c.encode_utf8(&mut [0; 4]))
}

fn sentence_start(text: &str, tokens: &[Token<'_>], index: usize) -> bool {
    index == 0 || {
        let gap = &text[tokens[index - 1].end..tokens[index].start];
        gap.contains('\n') || re!(r"\A[.!?]+[ \t]*\z").is_match(gap)
    }
}

fn ordinary_gap(text: &str, tokens: &[Token<'_>], first: usize, last: usize) -> bool {
    (first..last).all(|i| {
        text[tokens[i].end..tokens[i + 1].start]
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | ','))
    })
}

fn escaped_at(text: &str, position: usize) -> bool {
    text.as_bytes()[..position]
        .iter()
        .rev()
        .take_while(|&&c| c == b'\\')
        .count()
        % 2
        == 1
}

fn quote_code_spans(text: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte = |index: usize| chars.get(index).map_or(text.len(), |&(offset, _)| offset);
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let opening = chars[i].1;
        if opening == '`' {
            while i < chars.len() && chars[i].1 == '`' {
                i += 1;
            }
            let width = i - start;
            let mut end = chars.len();
            while i < chars.len() {
                if chars[i].1 != '`' {
                    i += 1;
                    continue;
                }
                let run = i;
                while i < chars.len() && chars[i].1 == '`' {
                    i += 1;
                }
                if i - run == width && !escaped_at(text, byte(run)) {
                    end = i;
                    break;
                }
            }
            spans.push(byte(start)..byte(end));
            i = end;
            continue;
        }
        let closing = match opening {
            '"' => '"',
            '“' => '”',
            '‘' => '’',
            '\'' => '\'',
            _ => {
                i += 1;
                continue;
            }
        };
        if opening == '\'' && start > 0 && word_char(chars[start - 1].1) {
            i += 1;
            continue;
        }
        i += 1;
        let mut end = chars.len();
        while i < chars.len() {
            if chars[i].1 == '\\' {
                i = (i + 2).min(chars.len());
                continue;
            }
            let inside_word = matches!(closing, '\'' | '’')
                && i > 0
                && i + 1 < chars.len()
                && word_char(chars[i - 1].1)
                && word_char(chars[i + 1].1);
            if chars[i].1 == closing && !inside_word {
                end = i + 1;
                break;
            }
            i += 1;
        }
        spans.push(byte(start)..byte(end));
        i = end;
    }
    spans
}

fn canonical_quotes(text: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    for span in quote_code_spans(text) {
        output.push_str(&text[cursor..span.start]);
        let value = &text[span.clone()];
        let mut changed = false;
        for (open, close, replacement) in [('“', '”', '"'), ('‘', '’', '\'')] {
            if value.starts_with(open)
                && value.ends_with(close)
                && value.len() >= open.len_utf8() + close.len_utf8()
                && !escaped_at(value, value.len() - close.len_utf8())
            {
                output.push(replacement);
                output.push_str(&value[open.len_utf8()..value.len() - close.len_utf8()]);
                output.push(replacement);
                changed = true;
                break;
            }
        }
        if !changed {
            output.push_str(value);
        }
        cursor = span.end;
    }
    output.push_str(&text[cursor..]);
    output
}

type Repeat = Vec<Range<usize>>;

fn repeat_groups(text: &str, tokens: &[Token<'_>]) -> Vec<Repeat> {
    let mut groups = Vec::new();
    for size in 1..=4 {
        if tokens.len() < 2 * size {
            continue;
        }
        for start in 0..=tokens.len() - 2 * size {
            if size == 1 && tokens[start].in_set(SINGLE_REPEAT_PROTECTED) {
                continue;
            }
            if size > 1 && (start + 1..start + size).all(|i| tokens[i].is(tokens[start].value)) {
                continue;
            }
            let matches = |other: usize| {
                other + size <= tokens.len()
                    && (0..size).all(|i| tokens[start + i].is(tokens[other + i].value))
            };
            if !matches(start + size) {
                continue;
            }
            let mut end = start + 2 * size;
            while matches(end) {
                end += size;
            }
            if ordinary_gap(text, tokens, start, end - 1) {
                groups.push((start..end).step_by(size).map(|i| i..i + size).collect());
            }
        }
    }
    groups
}

fn restart_groups(text: &str, tokens: &[Token<'_>]) -> Vec<(Range<usize>, usize)> {
    let mut groups = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !token.in_set(ARTICLES) {
            continue;
        }
        let mut j = i + 1;
        while j < tokens.len() && tokens[j].in_set(HESITATIONS) && j - i <= 4 {
            j += 1;
        }
        let count = j - i - 1;
        if !(1..=4).contains(&count) {
            continue;
        }
        if j < tokens.len() && tokens[j].is("yeah") {
            if count < 2
                || j + 1 >= tokens.len()
                || !text[tokens[j].end..tokens[j + 1].start].contains(',')
            {
                continue;
            }
            j += 1;
        }
        if j < tokens.len() && tokens[j].in_set(DETERMINERS) && ordinary_gap(text, tokens, i, j) {
            groups.push((i..j, j));
        }
    }
    groups
}

#[derive(Clone, Copy, PartialEq)]
enum Protection {
    Word,
    Exact,
    Quote,
}

struct Protected {
    span: Range<usize>,
    kind: Protection,
}

fn literal_context_spans(text: &str, tokens: &[Token<'_>]) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if token.in_set(LITERAL_NOUNS) {
            let mut start = i + 1;
            if start < tokens.len()
                && (tokens[start].in_set(&["literal", "value"])
                    || (token.in_set(&["label", "labels"])
                        && tokens[start].in_set(&["reads", "says", "is", "was"])))
            {
                start += 1;
            } else if start + 1 < tokens.len()
                && tokens[start].in_set(&["starts", "ends"])
                && tokens[start + 1].in_set(&["in", "with"])
            {
                start += 2;
            }
            if start < tokens.len() {
                let plain = text[token.end..tokens[start].start]
                    .chars()
                    .all(|c| matches!(c, ' ' | '\t' | ':'));
                if !plain && (start == i + 1 || !ordinary_gap(text, tokens, i, start)) {
                    continue;
                }
                let mut end = start + 1;
                while end < (start + 4).min(tokens.len()) && tokens[end].is(tokens[start].value) {
                    end += 1;
                }
                if ordinary_gap(text, tokens, start, end - 1) {
                    spans.push(tokens[start].start..tokens[end - 1].end);
                }
            }
        }
        if token.is("notation") {
            for end in i + 2..(i + 6).min(tokens.len()) {
                if tokens[end].in_set(&["means", "denotes", "represents"]) {
                    if text[token.end..tokens[i + 1].start]
                        .chars()
                        .all(|c| matches!(c, ' ' | '\t' | ',' | ':'))
                        && ordinary_gap(text, tokens, i + 1, end)
                    {
                        spans.push(tokens[i + 1].start..tokens[end - 1].end);
                    }
                    break;
                }
            }
        }
        if token.in_set(&["said", "answered", "replied", "uttered", "responded"]) {
            let start = i + 1;
            let mut end = start;
            while end < (start + 4).min(tokens.len()) && tokens[end].in_set(HESITATIONS) {
                end += 1;
            }
            if end == start || !ordinary_gap(text, tokens, i, end - 1) {
                continue;
            }
            let window_end = (end + 12).min(tokens.len());
            let boundary = (end..window_end)
                .find(|&j| text[tokens[j - 1].end..tokens[j].start].contains(['.', '!', '?', '\n']))
                .unwrap_or(window_end);
            let cues = &tokens[end..boundary];
            if cues
                .iter()
                .any(|t| t.in_set(&["sound", "utterance", "syllable"]))
                && cues
                    .iter()
                    .any(|t| t.in_set(&["exact", "literal", "recorded", "transcribed"]))
            {
                spans.push(tokens[start].start..tokens[end - 1].end);
            }
        }
    }
    spans
}

fn protected_spans(
    text: &str,
    tokens: &[Token<'_>],
    terms: &[String],
    repeats: &[Repeat],
) -> Result<Vec<Protected>, String> {
    let mut spans: Vec<_> = quote_code_spans(text)
        .into_iter()
        .map(|span| Protected {
            span,
            kind: Protection::Quote,
        })
        .collect();
    // Python \s additionally recognizes U+001C..001F. Identifier alternatives
    // ending in a word character get the original Python word-boundary check.
    let identifiers = re!(
        r"(?P<loose>https?://[^\s\x1c-\x1f]+|www\.[^\s\x1c-\x1f]+|[^\s\x1c-\x1f@]+@[^\s\x1c-\x1f@]+)|(?P<word>[\p{L}\p{N}_]+(?:[._:/+\-][\p{L}\p{N}_]+)+|[\p{L}\p{N}_]*[\p{Nd}_][\p{L}\p{N}_]*)"
    );
    for capture in identifiers.captures_iter(text) {
        let m = capture.get(0).expect("identifier capture");
        if capture.name("word").is_some()
            && (text[..m.start()].chars().next_back().is_some_and(word_char)
                || text[m.end()..].chars().next().is_some_and(word_char))
        {
            continue;
        }
        spans.push(Protected {
            span: m.range(),
            kind: Protection::Exact,
        });
    }
    for term in terms.iter().filter(|term| !term.is_empty()) {
        // Python re.IGNORECASE has the dotted/dotless I equivalence in addition
        // to Unicode simple folding; it deliberately does not expand ß to ss.
        let pattern: String = term
            .chars()
            .map(|c| {
                if matches!(c, 'i' | 'I' | 'İ' | 'ı') {
                    "[iIİı]".to_string()
                } else {
                    regex::escape(&c.to_string())
                }
            })
            .collect();
        let pattern = Regex::new(&format!("(?i:{pattern})"))
            .map_err(|_| "Protected term pattern limit exceeded".to_string())?;
        let mut search = 0;
        while let Some(m) = pattern.find_at(text, search) {
            if text[..m.start()].chars().next_back().is_some_and(word_char)
                || text[m.end()..].chars().next().is_some_and(word_char)
            {
                // A failed lookaround must not consume the whole match: the
                // next valid phrase can overlap it ("sum um um", "um um").
                search = m.start()
                    + text[m.start()..]
                        .chars()
                        .next()
                        .expect("nonempty term")
                        .len_utf8();
                continue;
            }
            spans.push(Protected {
                span: m.range(),
                kind: Protection::Exact,
            });
            search = m.end();
        }
    }
    let letter = re!(r"\A\p{L}\z");
    let period_gap = re!(r"\A\.[ \t]*\z");
    for (i, token) in tokens.iter().enumerate() {
        let lowercase = token.value.to_lowercase();
        let mut lock =
            token.in_set(NEGATIONS) || lowercase.ends_with("n't") || lowercase.ends_with("n’t");
        lock |= token.in_set(UNITS) && i > 0 && tokens[i - 1].value.chars().any(python_digit);
        let capital = token.value.chars().next().is_some_and(upper);
        if i > 0 && capital {
            let prior = tokens[i - 1];
            let initial = prior.value.chars().count() == 1
                && letter.is_match(prior.value)
                && prior.value.chars().next().is_some_and(upper);
            lock |= (prior.in_set(HONORIFICS) || initial)
                && period_gap.is_match(&text[prior.end..token.start]);
        }
        let initial_restart = sentence_start(text, tokens, i)
            && repeats.iter().any(|group| {
                group.iter().any(|block| {
                    block.len() > 1
                        && block.start == i
                        && group.iter().any(|other| {
                            other != block
                                && tokens[other.start].value == token.value.to_lowercase()
                        })
                })
            });
        let common_initial_filler = sentence_start(text, tokens, i) && token.in_set(HESITATIONS);
        lock |=
            capital && !token.in_set(FUNCTION_WORDS) && !common_initial_filler && !initial_restart;
        if lock {
            spans.push(Protected {
                span: token.start..token.end,
                kind: Protection::Word,
            });
        }
        if token.in_set(MENTION_MARKERS) {
            let end = text[token.end..]
                .find(['.', '!', '?', '\n'])
                .map_or(text.len(), |end| token.end + end);
            spans.push(Protected {
                span: token.start..end,
                kind: Protection::Word,
            });
        }
    }
    spans.extend(
        literal_context_spans(text, tokens)
            .into_iter()
            .map(|span| Protected {
                span,
                kind: Protection::Word,
            }),
    );
    Ok(spans)
}

struct DashPair {
    span: Range<usize>,
    left: usize,
    right: usize,
}

fn paired_filler_dashes(text: &str, tokens: &[Token<'_>], deleted: &[bool]) -> Vec<DashPair> {
    let mut pairs = Vec::new();
    for capture in re!(r"—([^—\n]*)—").captures_iter(text) {
        let whole = capture.get(0).expect("dash capture");
        let inner = capture.get(1).expect("dash payload");
        let inside: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| t.start >= inner.start() && t.end <= inner.end())
            .map(|(i, _)| i)
            .collect();
        if !(1..=4).contains(&inside.len())
            || inside
                .iter()
                .any(|&i| !deleted[i] || !tokens[i].in_set(HESITATIONS))
        {
            continue;
        }
        if !word_pattern()
            .replace_all(inner.as_str(), "")
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | ','))
        {
            continue;
        }
        if inside[0] == 0 {
            continue;
        }
        let left = inside[0] - 1;
        let right = inside[inside.len() - 1] + 1;
        if right >= tokens.len() || deleted[left] || deleted[right] {
            continue;
        }
        let (mut start, mut end) = (whole.start(), whole.end());
        while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        while end < text.len() && matches!(text.as_bytes()[end], b' ' | b'\t') {
            end += 1;
        }
        if start == tokens[left].end && end == tokens[right].start {
            pairs.push(DashPair {
                span: start..end,
                left,
                right,
            });
        }
    }
    pairs
}

fn reconstruct(
    text: &str,
    tokens: &[Token<'_>],
    deleted: &[bool],
) -> Option<(String, Vec<DashPair>)> {
    if deleted.iter().all(|&d| d) {
        return (text
            .chars()
            .all(|c| word_char(c) || python_space(c) || matches!(c, ',' | '.'))
            && !text.contains('\n'))
        .then(|| (String::new(), Vec::new()));
    }
    let pairs = paired_filler_dashes(text, tokens, deleted);
    let mut spans: Vec<_> = pairs.iter().map(|p| p.span.clone()).collect();
    for (i, token) in tokens.iter().enumerate().filter(|(i, _)| deleted[*i]) {
        let (mut start, mut end) = (token.start, token.end);
        if pairs
            .iter()
            .any(|p| p.span.start <= start && end <= p.span.end)
        {
            continue;
        }
        if text.as_bytes().get(end) == Some(&b',') {
            end += 1;
        }
        while end < text.len() && matches!(text.as_bytes()[end], b' ' | b'\t') {
            end += 1;
        }
        if deleted[i..].iter().all(|&d| d) {
            while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
                start -= 1;
            }
            if start > 0 && text.as_bytes()[start - 1] == b',' {
                start -= 1;
            }
        }
        spans.push(start..end);
    }
    spans.sort_by_key(|s| (s.start, s.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut().filter(|last| span.start <= last.end) {
            last.end = last.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    let mut output = String::new();
    let mut position = 0;
    for span in merged {
        output.push_str(&text[position..span.start]);
        if pairs
            .iter()
            .any(|p| span.start <= p.span.start && p.span.end <= span.end)
        {
            output.push(' ');
        }
        position = span.end;
    }
    output.push_str(&text[position..]);
    Some((output, pairs))
}

fn proposal_local_gaps(
    proposal: &str,
    proposed: &[Token<'_>],
    kept: &[usize],
    pairs: &[DashPair],
) -> Option<String> {
    let mut changes = Vec::new();
    for pair in pairs {
        let a = kept.iter().position(|&i| i == pair.left)?;
        let b = kept.iter().position(|&i| i == pair.right)?;
        if b != a + 1 {
            return None;
        }
        let span = proposed[a].end..proposed[b].start;
        let gap = &proposal[span.clone()];
        if !gap.chars().all(|c| matches!(c, ' ' | '\t' | ',' | '—')) || gap.matches('—').count() > 2
        {
            return None;
        }
        changes.push(span);
    }
    let mut output = proposal.to_string();
    for span in changes.into_iter().rev() {
        output.replace_range(span, " ");
    }
    Some(output)
}

fn punctuation_signature(text: &str, keep_commas: bool) -> String {
    let canonical = canonical_quotes(text);
    let canonical = if keep_commas {
        re!(r"[ \t]*,[ \t]*")
            .replace_all(&canonical, ",")
            .into_owned()
    } else {
        canonical.replace(',', " ")
    };
    let collapsed = re!(r"[ \t]+").replace_all(&canonical, " ");
    let mut signature = collapsed.trim_matches([' ', '\t']).to_string();
    if signature.ends_with('.') && !signature.ends_with("..") {
        signature.pop();
    }
    signature
}

/// Validate a *completed* model response and return only source-derived text.
/// The caller must retain the original transcript on any error, and must pass
/// the vocabulary/replacement values from the same recording settings snapshot.
pub fn validate(
    source: &str,
    proposal: &str,
    protected_terms: &[String],
) -> Result<String, String> {
    if source.len().max(proposal.len()) > MAX_CLEANUP_BYTES {
        return Err("Input/output byte limit exceeded".into());
    }
    let tokens = tokenize(source);
    let proposed = tokenize(proposal);
    if source == proposal {
        return Ok(source.to_string());
    }
    if tokens.is_empty() || tokens.len() > MAX_CLEANUP_WORDS || proposed.len() > tokens.len() {
        return Err("Word limit or word addition".into());
    }
    let repeats = repeat_groups(source, &tokens);
    let protected = protected_spans(source, &tokens, protected_terms, &repeats)?;
    let locked: Vec<_> = tokens
        .iter()
        .map(|t| {
            protected
                .iter()
                .any(|p| t.start < p.span.end && t.end > p.span.start)
        })
        .collect();
    for protection in &protected {
        if protection.kind == Protection::Word {
            continue;
        }
        let value = &source[protection.span.clone()];
        let changed = if protection.kind == Protection::Quote {
            let value = canonical_quotes(value);
            canonical_quotes(proposal).matches(&value).count()
                < canonical_quotes(source).matches(&value).count()
        } else {
            proposal.matches(value).count() < source.matches(value).count()
        };
        if changed {
            return Err("Protected span changed".into());
        }
    }
    let restarts = restart_groups(source, &tokens);
    let mut possible: Vec<_> = tokens.iter().map(|t| t.in_set(HESITATIONS)).collect();
    for group in &repeats {
        for block in group {
            for i in block.clone() {
                possible[i] = true;
            }
        }
    }
    for (group, _) in &restarts {
        for i in group.clone() {
            possible[i] = true;
        }
    }
    for (i, lock) in locked.into_iter().enumerate() {
        possible[i] &= !lock;
    }
    let initial_cap = |source: &str, output: &str| {
        source
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
            && output
                == format!(
                    "{}{}",
                    (source.as_bytes()[0] as char).to_ascii_uppercase(),
                    &source[1..]
                )
    };
    let mut stack = vec![(0, 0, Vec::new())];
    let mut alignments = Vec::new();
    let mut states = 0;
    while let Some((i, j, kept)) = stack.pop() {
        states += 1;
        if states > MAX_ALIGNMENT_STATES {
            return Err("Alignment work limit exceeded".into());
        }
        if tokens.len() - i < proposed.len() - j {
            continue;
        }
        if i == tokens.len() {
            if j == proposed.len() {
                if alignments.len() >= MAX_ALIGNMENTS {
                    return Err("Alignment count limit exceeded".into());
                }
                alignments.push(kept);
            }
            continue;
        }
        if possible[i] {
            stack.push((i + 1, j, kept.clone()));
        }
        if j < proposed.len()
            && tokens[i].is(proposed[j].value)
            && (tokens[i].value == proposed[j].value
                || initial_cap(tokens[i].value, proposed[j].value))
        {
            let mut kept = kept;
            kept.push(i);
            stack.push((i + 1, j + 1, kept));
        }
    }
    let mut accepted = BTreeMap::<String, bool>::new();
    for kept in alignments {
        let mut deleted = vec![true; tokens.len()];
        for &i in &kept {
            deleted[i] = false;
        }
        let mut allowed: Vec<_> = tokens
            .iter()
            .enumerate()
            .map(|(i, t)| deleted[i] && t.in_set(HESITATIONS))
            .collect();
        for group in &repeats {
            if group.iter().any(|block| block.clone().all(|i| !deleted[i])) {
                for block in group
                    .iter()
                    .filter(|block| (**block).clone().all(|i| deleted[i]))
                {
                    for i in block.clone() {
                        allowed[i] = true;
                    }
                }
            }
        }
        for (group, replacement) in &restarts {
            if group.clone().all(|i| deleted[i]) && !deleted[*replacement] {
                for i in group.clone() {
                    allowed[i] = true;
                }
            }
        }
        if deleted != allowed {
            continue;
        }
        let Some((mut output, pairs)) = reconstruct(source, &tokens, &deleted) else {
            continue;
        };
        let output_tokens = tokenize(&output);
        if output_tokens.len() != kept.len() {
            continue;
        }
        let mut caps = Vec::new();
        let mut invalid_case = false;
        for (j, &i) in kept.iter().enumerate() {
            if tokens[i].value != proposed[j].value {
                if !sentence_start(&output, &output_tokens, j) {
                    invalid_case = true;
                    break;
                }
                caps.push((output_tokens[j].start, proposed[j].value[..1].to_string()));
            }
        }
        if invalid_case {
            continue;
        }
        for (position, replacement) in caps.into_iter().rev() {
            output.replace_range(position..position + 1, &replacement);
        }
        let Some(comparison) = proposal_local_gaps(proposal, &proposed, &kept, &pairs) else {
            continue;
        };
        if punctuation_signature(&output, false) != punctuation_signature(&comparison, false) {
            continue;
        }
        let comma_match =
            punctuation_signature(&output, true) == punctuation_signature(&comparison, true);
        *accepted.entry(output).or_default() |= comma_match;
    }
    if accepted.is_empty() {
        return Err("Proposal requires protected/unsupported edits or punctuation changes".into());
    }
    if accepted.len() != 1 {
        accepted.retain(|_, comma_match| *comma_match);
    }
    if accepted.len() != 1 {
        return Err("Ambiguous source reconstruction".into());
    }
    Ok(accepted
        .into_keys()
        .next()
        .expect("one accepted reconstruction"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_frozen_python_development_oracle() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/cleanup-validation-v6.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert!(
            cases.len() > 100,
            "parity corpus must include authored and model proposals"
        );
        for (index, case) in cases.iter().enumerate() {
            let terms: Vec<String> =
                serde_json::from_value(case["protected_terms"].clone()).unwrap();
            let result = validate(
                case["source"].as_str().unwrap(),
                case["proposal"].as_str().unwrap(),
                &terms,
            );
            assert_eq!(
                result.is_ok(),
                case["accepted"].as_bool().unwrap(),
                "case {index}: {case}"
            );
            if let Ok(output) = result {
                assert_eq!(
                    output,
                    case["output"].as_str().unwrap(),
                    "case {index}: {case}"
                );
            }
        }
    }

    #[test]
    fn preserves_source_punctuation_and_protected_content() {
        assert_eq!(
            validate("Uh, not yet.", "Not yet.", &[]).unwrap(),
            "Not yet."
        );
        assert!(validate("Dr. Um um approved it.", "Dr. approved it.", &[]).is_err());
        assert!(validate(
            "Keep \"um,\nuh next\" verbatim.",
            "Keep \"next\" verbatim.",
            &[]
        )
        .is_err());
        assert!(validate("The parameter um is fixed.", "The parameter is fixed.", &[]).is_err());
        assert_eq!(
            validate("I, I need the form.", "I need the form.", &[]).unwrap(),
            "I need the form."
        );
    }

    #[test]
    fn rejects_resource_limits_before_alignment() {
        assert!(validate(&"x".repeat(MAX_CLEANUP_BYTES + 1), "", &[]).is_err());
        assert!(validate(&"um ".repeat(MAX_CLEANUP_WORDS + 1), "", &[]).is_err());
        assert!(validate(&"um ".repeat(100), &"um ".repeat(50), &[]).is_err());
    }

    #[test]
    fn overlapping_invalid_dictionary_match_does_not_hide_a_valid_phrase() {
        assert!(validate("sum um um", "sum", &["um um".into()]).is_err());
        assert!(validate("éum um um", "éum", &["um um".into()]).is_err());
    }

    #[test]
    fn unicode_casefold_and_dictionary_boundaries_match_python_oracle() {
        assert_eq!(
            validate("Straße strasse ready.", "Straße ready.", &[]).unwrap(),
            "Straße ready."
        );
        assert_eq!(
            validate("We σς σσ paused.", "We σς paused.", &[]).unwrap(),
            "We σς paused."
        );
        assert_eq!(validate("ß um", "ß", &["ss um".into()]).unwrap(), "ß");
        assert!(validate("ı um", "ı", &["I um".into()]).is_err());
        assert!(validate("um\u{301} hi", "\u{301} hi", &["um".into()]).is_err());
    }
}
