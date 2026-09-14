//! Source-preserving validation of a completed cleanup proposal.
//!
//! Policy port of frozen experimental v6 (SHA256 4a8bc2a49feb6b944e765ec238f7bdc381a65576907d1b8ddddf9a4b172b177b).
//! The model chooses edits; this module permits only bounded source deletions
//! and reconstructs the delivered text from the source. Rejection must retain
//! the original transcript. Syntactic guards do not establish semantic safety.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::OnceLock;

pub use focaccia::unicode_full_case_eq;
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

/// One repeat run at a fixed block width: blocks are `start..start+width`,
/// `start+width..start+2*width`, … while `< end`. Three usizes per run; the
/// blocks are derived lazily so a run of k repetitions costs nothing per
/// block.
struct RepeatGroup {
    width: usize,
    start: usize,
    end: usize,
}

impl RepeatGroup {
    fn blocks(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        (self.start..self.end)
            .step_by(self.width)
            .map(|block| block..block + self.width)
    }
}

/// Maximal validated repeat runs, width-major then start-ascending (the
/// order callers' protections and authorizations were written against;
/// distinct phases are never merged). For each width, every residue class is
/// walked once over the adjacency `block(s) == block(s + width)`: a maximal
/// true-run makes every covered start a repeat ending two blocks past the
/// run, so no candidate scans to its own maximal end and no dominated suffix
/// subgroup is materialized.
fn repeat_groups(text: &str, tokens: &[Token<'_>]) -> Vec<RepeatGroup> {
    let n = tokens.len();
    // Prefix sums of non-ordinary gaps: `bad[end - 1] == bad[start]` is
    // exactly `ordinary_gap(text, tokens, start, end - 1)` from the frozen
    // scan.
    let mut bad = vec![0usize; n];
    for i in 0..n.saturating_sub(1) {
        let ordinary = text[tokens[i].end..tokens[i + 1].start]
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | ','));
        bad[i + 1] = bad[i] + usize::from(!ordinary);
    }
    let mut groups = Vec::new();
    for width in 1..=4usize {
        if n < 2 * width {
            continue;
        }
        let last_start = n - 2 * width;
        let block_eq = |a: usize, b: usize| {
            (0..width).all(|i| tokens[a + i].is(tokens[b + i].value))
        };
        // `end_of[s]`: exclusive end of the maximal equal-block run covering
        // start `s` (0 = no repeat starts there).
        let mut end_of = vec![0usize; last_start + 1];
        for residue in 0..width {
            let mut run_start = usize::MAX;
            let mut k = residue;
            while k <= last_start {
                if block_eq(k, k + width) {
                    if run_start == usize::MAX {
                        run_start = k;
                    }
                } else if run_start != usize::MAX {
                    for s in (run_start..k).step_by(width) {
                        end_of[s] = k + width;
                    }
                    run_start = usize::MAX;
                }
                k += width;
            }
            if run_start != usize::MAX {
                for s in (run_start..k).step_by(width) {
                    end_of[s] = k + width;
                }
            }
        }
        // A start whose whole range lies inside an earlier EMITTED run of the
        // same width and phase is dominated by it: same phase means its
        // blocks are a subset, so any kept-sibling authorization or restart
        // witness it could produce, the covering run produces too. `covered`
        // records emitted runs only — a maximal run rejected by its own gap
        // check (a sentence break inside) must never shadow the first
        // gap-ordinary suffix run behind it.
        let mut covered = vec![0usize; width];
        for start in 0..=last_start {
            if width == 1 && tokens[start].in_set(SINGLE_REPEAT_PROTECTED) {
                continue;
            }
            if width > 1
                && (start + 1..start + width).all(|i| tokens[i].is(tokens[start].value))
            {
                continue;
            }
            let end = end_of[start];
            if end == 0 || end <= covered[start % width] {
                continue;
            }
            if bad[end - 1] != bad[start] {
                continue;
            }
            covered[start % width] = end;
            groups.push(RepeatGroup { width, start, end });
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

#[derive(Clone, Copy, Debug, PartialEq)]
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
    repeats: &[RepeatGroup],
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
                group.width > 1
                    && group.blocks().any(|block| block.start == i)
                    && group.blocks().any(|other| {
                        other.start != i
                            && tokens[other.start].value == token.value.to_lowercase()
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
    // The per-token suffix test inside the loop ("is everything from `i` on
    // deleted?") is true exactly when `i` is past the last kept token; one
    // rposition keeps reconstruction linear instead of O(tokens × flags).
    // (All-deleted returned above, so a kept token always exists.)
    let last_kept = deleted
        .iter()
        .rposition(|&d| !d)
        .expect("not all tokens deleted");
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
        if i > last_kept {
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
    validate_inner(source, proposal, protected_terms).map(|(output, _)| output)
}

/// `validate` plus the accepted alignment's deletion authority: one flag per
/// token of `word_spans(source)`, true = the frozen pass deleted that token.
/// Case substitutions are baked into the returned output; callers replay the
/// *output string* over the exact source bytes, never the flags alone.
fn validate_inner(
    source: &str,
    proposal: &str,
    protected_terms: &[String],
) -> Result<(String, Vec<bool>), String> {
    if source.len().max(proposal.len()) > MAX_CLEANUP_BYTES {
        return Err("Input/output byte limit exceeded".into());
    }
    let tokens = tokenize(source);
    let proposed = tokenize(proposal);
    if source == proposal {
        return Ok((source.to_string(), vec![false; tokens.len()]));
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
    // Shared immutable-source literal guard: every Exact/Quote span must
    // still satisfy its occurrence rule against the proposal (Word spans
    // stay enforced token-wise by `locked` above, exactly as this loop
    // always skipped them). One implementation for this helper, the deletion
    // adjudicator, and the caps renderer.
    if !LiteralGuard::from_protected(source, &protected).all_safe(proposal) {
        return Err("Protected span changed".into());
    }
    let restarts = restart_groups(source, &tokens);
    let mut possible: Vec<_> = tokens.iter().map(|t| t.in_set(HESITATIONS)).collect();
    for group in &repeats {
        possible[group.start..group.end].fill(true);
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
    let mut accepted = BTreeMap::<String, (bool, Vec<bool>)>::new();
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
            if group.blocks().any(|block| block.clone().all(|i| !deleted[i])) {
                for block in group.blocks().filter(|block| block.clone().all(|i| deleted[i])) {
                    for i in block {
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
        match accepted.entry(output) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().0 |= comma_match;
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((comma_match, deleted));
            }
        }
    }
    if accepted.is_empty() {
        return Err("Proposal requires protected/unsupported edits or punctuation changes".into());
    }
    if accepted.len() != 1 {
        accepted.retain(|_, (comma_match, _)| *comma_match);
    }
    if accepted.len() != 1 {
        return Err("Ambiguous source reconstruction".into());
    }
    let (output, (_, deleted)) = accepted
        .into_iter()
        .next()
        .expect("one accepted reconstruction");
    Ok((output, deleted))
}

/// Byte ranges of the word tokens in `text` — the same tokenizer the frozen
/// validator uses, so token indices and deletion flags align across calls.
pub fn word_spans(text: &str) -> Vec<Range<usize>> {
    tokenize(text).into_iter().map(|t| t.start..t.end).collect()
}

/// Token indices where a new sentence starts, using the frozen
/// `sentence_start` gap semantics verbatim (newline gap, or a whitespace-only
/// [.!?] run before the token; index 0 included). Tokenization is the frozen
/// tokenizer, so returned indices align with `word_spans(text)`.
pub fn sentence_boundaries(text: &str) -> Vec<usize> {
    let tokens = tokenize(text);
    (0..tokens.len()).filter(|&i| sentence_start(text, &tokens, i)).collect()
}

/// The window planner's caps extraction: given the frozen validator's OWN
/// authorized candidate (`candidate` accepted for `source` with exactly
/// `deleted` token deletions), return the capitalization flips it baked into
/// the candidate, as (source token index over `word_spans(source)`, letter).
/// A flip must have the `initial_cap` shape the frozen pass permits (ASCII
/// lowercase first byte → ASCII uppercase, remainder byte-identical); ANY
/// other token-text difference rejects the extraction (`None`) so the window
/// caches deletions only. Returns `None` if the candidate does not align
/// with `deleted` at all.
pub fn caps_flips(source: &str, candidate: &str, deleted: &[bool]) -> Option<Vec<(usize, char)>> {
    let tokens = tokenize(source);
    if tokens.len() != deleted.len() {
        return None;
    }
    let out_tokens = tokenize(candidate);
    let kept: Vec<usize> = (0..tokens.len()).filter(|&i| !deleted[i]).collect();
    if out_tokens.len() != kept.len() {
        return None;
    }
    let mut flips = Vec::new();
    for (j, &i) in kept.iter().enumerate() {
        let src = &source[tokens[i].start..tokens[i].end];
        let out = out_tokens[j].value;
        if src == out {
            continue;
        }
        let first = *src.as_bytes().first()?;
        if !first.is_ascii_lowercase() {
            return None;
        }
        let upper = (first as char).to_ascii_uppercase();
        if out != format!("{upper}{}", &src[1..]) {
            return None;
        }
        flips.push((i, upper));
    }
    Some(flips)
}

/// Apply window-authorized capitalization flips against the WHOLE final
/// text, using the original source as the immutable protection authority.
/// Each flip is a (token index over `word_spans(source)`, letter) pair a
/// frozen validator pass accepted inside its own window. A flip survives
/// only if: the token is globally kept; the source token has the ASCII
/// `initial_cap` shape and matches the recorded letter; its span overlaps no
/// Exact/Quote protection of the ORIGINAL source (`Word` protections permit
/// a sentence-initial capital exactly as the frozen pass does — proven
/// oracle-equivalent by probe, docs/journals/2026-09-12); and
/// `sentence_start` holds at the token's rank in the deletion-reconstructed
/// output (fillers before a sentence start may be deleted, exposing the cap
/// legally); and the WHOLE rendered text must still satisfy the shared
/// `LiteralGuard` occurrence rules of the source's Exact/Quote protections —
/// a flip that re-cases a literal another protection requires (e.g. a far
/// standalone term "in" makes the "in" inside "inside" required) reverts on
/// the spot while every other flip and the deletions stand. Surviving flips
/// are applied descending by byte to the reconstructed deletion output —
/// `source` itself is never mutated and no flip can capitalize inside quoted
/// material the window could not see.
pub fn authorized_caps(
    source: &str,
    deleted: &[bool],
    flips: &[(usize, char)],
    terms: &[String],
) -> String {
    let tokens = tokenize(source);
    let fallback = source.to_string();
    if tokens.len() != deleted.len() {
        return fallback;
    }
    let Some((base, _)) = reconstruct(source, &tokens, deleted) else {
        return fallback;
    };
    if flips.is_empty() {
        return base;
    }
    let out_tokens = tokenize(&base);
    let kept: Vec<usize> = (0..tokens.len()).filter(|&i| !deleted[i]).collect();
    if out_tokens.len() != kept.len() {
        // The reconstruction no longer mirrors the kept-token sequence;
        // drop ALL caps fail-closed (the deletions themselves stand).
        return base;
    }
    let repeats = repeat_groups(source, &tokens);
    let Ok(protected) = protected_spans(source, &tokens, terms, &repeats) else {
        return base;
    };
    let guard = LiteralGuard::from_protected(source, &protected);
    let mut apply: Vec<(usize, char)> = Vec::new();
    for &(token, letter) in flips {
        let Some(t) = tokens.get(token) else { continue };
        let Some(rank) = kept.binary_search(&token).ok() else { continue };
        let value = &source[t.start..t.end];
        let Some(&first) = value.as_bytes().first() else { continue };
        if !first.is_ascii_lowercase() || (first as char).to_ascii_uppercase() != letter {
            continue;
        }
        if protected.iter().any(|p| {
            p.kind != Protection::Word && t.start < p.span.end && t.end > p.span.start
        }) {
            continue;
        }
        let out = &out_tokens[rank];
        if out.value != value || !sentence_start(&base, &out_tokens, rank) {
            continue;
        }
        apply.push((out.start, letter));
    }
    apply.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
    apply.dedup_by(|a, b| a.0 == b.0);
    let mut output = base;
    for (position, letter) in apply {
        // ASCII first byte of a kept word token: exactly one byte wide.
        let original = output.as_bytes()[position];
        output.replace_range(position..position + 1, &letter.to_string());
        // Same immutable-source literal guard as the deletion path, checked
        // over the whole candidate after each flip: a flip that drops an
        // Exact/Quote literal below its source occurrence count (e.g. a far
        // standalone term "in" that makes the "in" inside "inside" required,
        // or quote bytes canonicalised out of the quote) reverts on the
        // spot; every other flip and the deletions stand. A whole-candidate
        // check (not a local window) also catches two flips jointly erasing
        // one occurrence of the same literal.
        if !guard.all_safe(&output) {
            output.replace_range(position..position + 1, &(original as char).to_string());
        }
    }
    output
}

/// A validated cleanup result plus the frozen pass's edit authority.
#[derive(Clone, Debug)]
pub struct CleanupEdits {
    /// Source-derived output text (reconstruction is the validator's, never
    /// the proposal's).
    pub output: String,
    /// One flag per token of `word_spans(source)`: true = the accepted
    /// alignment deleted that token. Case substitutions are already baked
    /// into `output`; they need no separate representation because replay
    /// replaces the *exact source bytes* with `output`.
    pub deleted: Vec<bool>,
}

/// `validate` plus the token-level deletion set.
pub fn validate_cleanup_with_edits(
    source: &str,
    proposal: &str,
    protected_terms: &[String],
) -> Result<CleanupEdits, String> {
    let reason = match validate_inner(source, proposal, protected_terms) {
        Ok((output, deleted)) => return Ok(CleanupEdits { output, deleted }),
        Err(reason) => reason,
    };
    if source.len().max(proposal.len()) > MAX_CLEANUP_BYTES {
        return Err(reason);
    }
    let tokens = tokenize(source);
    let proposed = tokenize(proposal);
    let (n, m) = (tokens.len(), proposed.len());
    if n == 0 || m == 0 || n.max(m) > MAX_CLEANUP_WORDS {
        return Err(reason);
    }
    // At most ~2 MiB. This alignment only proposes deletion runs; it does not
    // authorize them. Repeated/ambiguous words still pass the full validator.
    let width = m + 1;
    let mut lengths = vec![0u16; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lengths[i * width + j] = if tokens[i].is(proposed[j].value) {
                lengths[(i + 1) * width + j + 1] + 1
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j, mut start_i, mut start_j) = (0, 0, 0, 0);
    let mut runs = Vec::new();
    while i < n || j < m {
        if i < n && j < m && tokens[i].is(proposed[j].value) {
            if i > start_i && j == start_j {
                runs.push(start_i..i);
            }
            i += 1;
            j += 1;
            start_i = i;
            start_j = j;
        } else if i < n && (j == m || lengths[(i + 1) * width + j] >= lengths[i * width + j + 1]) {
            i += 1;
        } else {
            j += 1;
        }
    }
    if i > start_i && j == start_j {
        runs.push(start_i..i);
    }
    if runs.len() > 64 {
        return Err(reason);
    }
    let mut accepted = vec![false; n];
    for run in runs {
        let mut deleted = vec![false; n];
        deleted[run.clone()].fill(true);
        if let Some((candidate, _)) = reconstruct(source, &tokens, &deleted) {
            if validate(source, &candidate, protected_terms).is_ok() {
                accepted[run].fill(true);
            }
        }
    }
    if !accepted.iter().any(|&deleted| deleted) {
        return Err(reason);
    }
    let Some((candidate, _)) = reconstruct(source, &tokens, &accepted) else {
        return Err(reason);
    };
    // Composition can change literal/repeat context; validate the whole set.
    // The alignment's own deletion flags are the authority (composition can
    // re-align repeated words), so return them, not `accepted`.
    let (output, deleted) = validate_inner(source, &candidate, protected_terms)?;
    Ok(CleanupEdits { output, deleted })
}

/// Rebuild a candidate by deleting the flagged tokens from `source` using the
/// frozen validator's own erase/merge rules (trailing comma+space absorption,
/// leading-space trimming, double-space collapse). `None` on a length
/// mismatch between `deleted` and `word_spans(source)`.
pub fn deletions_candidate(source: &str, deleted: &[bool]) -> Option<String> {
    let tokens = tokenize(source);
    if tokens.len() != deleted.len() {
        return None;
    }
    reconstruct(source, &tokens, deleted).map(|(output, _)| output)
}

/// Immutable-source literal-preservation guard, the SAME rules `validate_inner`
/// applies to a proposal: `Word` spans stay enforced token-wise (the frozen
/// loop skips them), `Exact` values must still occur at least as often as in
/// the raw source, `Quote` values in the candidate's canonical form against
/// the canonical source count. `authorized_caps` and the deletion
/// adjudication reconstruct (or re-case) text the token-overlap protections
/// never see — gap-character terms (`.`, `,`, `...`), substring material
/// inside a longer word, or quoted material can lose bytes that no deleted
/// token overlaps — so both final renderers must pass here before their text
/// is accepted. Spans are deduplicated per distinct (value, kind): two spans
/// of one value are identical slices of the source, so their required counts
/// agree and one item decides the verdict for all. Each required count is
/// taken ONCE over the immutable source.
struct GuardItem {
    value: String,
    quoted: bool,
    need: usize,
}

struct LiteralGuard {
    items: Vec<GuardItem>,
}

impl LiteralGuard {
    /// Build from already-detected spans (the caps path has them for its
    /// overlap vetoes; one detection serves both). Word spans are skipped
    /// exactly as `validate_inner` skips them.
    fn from_protected(source: &str, protected: &[Protected]) -> Self {
        let mut items: Vec<GuardItem> = Vec::new();
        let mut canon_source: Option<String> = None;
        for protection in protected {
            if protection.kind == Protection::Word {
                continue;
            }
            let quoted = protection.kind == Protection::Quote;
            let value = if quoted {
                canonical_quotes(&source[protection.span.clone()])
            } else {
                source[protection.span.clone()].to_string()
            };
            if items
                .iter()
                .any(|g| g.value == value && g.quoted == quoted)
            {
                continue;
            }
            let need = if quoted {
                let canon = canon_source.get_or_insert_with(|| canonical_quotes(source));
                canon.matches(&value).count()
            } else {
                source.matches(value.as_str()).count()
            };
            items.push(GuardItem { value, quoted, need });
        }
        Self { items }
    }

    /// True iff one item still satisfies the frozen rule for its kind:
    /// Exact values are counted raw; Quote values are counted in the
    /// candidate's CANONICAL form against the canonical source count.
    /// `canon` caches the candidate's canonical form across items.
    fn item_ok(item: &GuardItem, output: &str, canon: &mut Option<String>) -> bool {
        let have = if item.quoted {
            let canonical = canon.get_or_insert_with(|| canonical_quotes(output));
            canonical.matches(&item.value).count()
        } else {
            output.matches(item.value.as_str()).count()
        };
        have >= item.need
    }

    /// True iff every required literal still satisfies its frozen rule.
    fn all_safe(&self, output: &str) -> bool {
        let mut canon = None;
        self.items.iter().all(|item| Self::item_ok(item, output, &mut canon))
    }
}

/// One `protected_spans` pass serving both deletion vetoes: the token-hit
/// mask and the deduplicated literal guard, computed together and cached.
struct CachedProtections {
    hit: Vec<bool>,
    guard: LiteralGuard,
}

/// Per-token eligibility view of a [`DeletionContext`] (spec §4.3): both
/// vectors align with `word_spans(source)` / the context's tokens.
/// `possible[i]` proves a frozen-rule deletion candidate EXISTS at token
/// `i`; `cap_opportunity[i]` marks a lowercase-start word at a caps-
/// exposed position. Used ONLY to prove windows skippable.
#[derive(Clone, Debug)]
pub struct WorkMask {
    pub possible: Vec<bool>,
    pub cap_opportunity: Vec<bool>,
}

/// The (source, terms)-only analysis behind [`validate_deletions`], prepared
/// once and reused for many deletion vectors over the same source. Tokenizing
/// and the repeat/restart/protection parsing depend only on the source and the
/// protected terms, never on the candidate flags; a composing caller (the
/// incremental correction's terminal gate) adjudicates one vector per accepted
/// window, so hoisting that work turns per-run cost into a single pass.
///
/// The fallible protection-pattern stage stays lazy: the frozen helper only
/// reaches `protected_spans` after the length-mismatch, all-false, and
/// all-true checks, and this context must reject (or accept) exactly when the
/// frozen helper does — so a term-pattern budget failure surfaces only on
/// vectors that would have reached that line, and the no-op all-false call
/// keeps returning `source` even with an over-budget term set.
pub struct DeletionContext<'a> {
    source: &'a str,
    terms: &'a [String],
    tokens: Vec<Token<'a>>,
    repeats: Vec<RepeatGroup>,
    restarts: Vec<(Range<usize>, usize)>,
    hesitation: Vec<bool>,
    protections: OnceLock<Result<CachedProtections, String>>,
}

impl<'a> DeletionContext<'a> {
    /// Analyze `source` for deletion vectors under `terms`. Infallible: the
    /// only erroring stage (term patterns) is deferred to first use, matching
    /// the frozen helper's precedence.
    pub fn prepare(source: &'a str, terms: &'a [String]) -> Self {
        let tokens = tokenize(source);
        let repeats = repeat_groups(source, &tokens);
        let restarts = restart_groups(source, &tokens);
        let hesitation = tokens.iter().map(|t| t.in_set(HESITATIONS)).collect();
        Self {
            source,
            terms,
            tokens,
            repeats,
            restarts,
            hesitation,
            protections: OnceLock::new(),
        }
    }

    fn protections(&self) -> Result<&CachedProtections, String> {
        self.protections
            .get_or_init(|| {
                let protected =
                    protected_spans(self.source, &self.tokens, self.terms, &self.repeats)?;
                let hit = (0..self.tokens.len())
                    .map(|i| {
                        let t = self.tokens[i];
                        protected
                            .iter()
                            .any(|p| t.start < p.span.end && t.end > p.span.start)
                    })
                    .collect();
                Ok(CachedProtections {
                    hit,
                    guard: LiteralGuard::from_protected(self.source, &protected),
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Adjudicate a deletion vector against the prepared source: identical
    /// outcome to `validate_deletions(self.source, deleted, self.terms)` —
    /// the group analysis is computed once and cached. An all-true vector is
    /// NOT blanket-rejected here: exactly as in `validate_inner`, the
    /// protected-hit / repeat-keep-one / restart-replacement / hesitation
    /// admissibility rules run first, and the shared `reconstruct` admits a
    /// wholly-unprotected-hesitation text to empty output (frozen legacy
    /// contract) while substantive or protected content cannot satisfy
    /// all-true admissibility.
    pub fn adjudicate(&self, deleted: &[bool]) -> Result<String, String> {
        if self.tokens.len() != deleted.len() {
            return Err("Deletion vector length mismatch".into());
        }
        if deleted.iter().all(|&d| !d) {
            return Ok(self.source.to_string());
        }
        let cached = self.protections()?;
        if deleted
            .iter()
            .zip(&cached.hit)
            .any(|(&d, &p)| d && p)
        {
            return Err("Protected token deleted".into());
        }
        let mut allowed: Vec<bool> = (0..self.tokens.len())
            .map(|i| deleted[i] && self.hesitation[i])
            .collect();
        for group in &self.repeats {
            if group.blocks().any(|block| block.clone().all(|i| !deleted[i])) {
                for block in group.blocks().filter(|block| block.clone().all(|i| deleted[i])) {
                    for i in block {
                        allowed[i] = true;
                    }
                }
            }
        }
        for (group, replacement) in &self.restarts {
            if group.clone().all(|i| deleted[i]) && !deleted[*replacement] {
                for i in group.clone() {
                    allowed[i] = true;
                }
            }
        }
        if deleted
            .iter()
            .zip(&allowed)
            .any(|(&d, &a)| d && !a)
        {
            return Err("Deletions require protected/unsupported edits".into());
        }
        let Some((output, _pairs)) = reconstruct(self.source, &self.tokens, deleted) else {
            return Err("Deletion reconstruction failed".into());
        };
        // Immutable-source literal guard: the reconstructed text must keep
        // every Exact/Quote literal occurrence the source has — the same
        // comparison `validate_inner` runs against the proposal, and placed
        // at the same point in the checks (protections detected → candidate
        // literals). It catches bytes erased OUTSIDE every deleted token:
        // gap-character terms (a terminal "." or "..." in an emptied or
        // re-flowed text) and substring material that no token overlaps.
        if !cached.guard.all_safe(&output) {
            return Err("Protected span changed".into());
        }
        // Mirror validate_inner's output-token-count gate (dash-pair/merge
        // surprises surface here rather than at the terminal validate). The
        // count uses the tokenizer's own pattern — identical token definition,
        // no Vec<Token> allocation per adjudicated run.
        if word_pattern().find_iter(&output).count()
            != self
                .tokens
                .iter()
                .enumerate()
                .filter(|(i, _)| !deleted[*i])
                .count()
        {
            return Err("Deletion reconstruction token count mismatch".into());
        }
        Ok(output)
    }

    /// The batch planner's eligibility proof (spec §4.3): per source token,
    /// `possible[i]` = a frozen-rule deletion candidate EXISTS at `i`
    /// (hesitation ∪ repeat-member ∪ restart-member, minus protected-hit),
    /// and `cap_opportunity[i]` = an ASCII-lowercase word a frozen
    /// caps-pass could legitimately flip at a sentence start exposed
    /// through fully-deletable fillers. Computed from THIS context's own
    /// structures (one tokenization, cached protections) — the same
    /// membership sets `adjudicate` consults; no second tokenizer, no new
    /// policy.
    /// A NECESSARY condition only — an OVER-APPROXIMATION of joint legality:
    /// membership is per-token, so a marked window may still earn ZERO legal
    /// edits once the joint repeat/restart/protection rules re-adjudicate
    /// (e.g. deleting the last kept copy of a repeat block). What the mask
    /// CAN prove is the absence direction: zero candidates ⇒ provably no
    /// work ⇒ skippable. The terminal `adjudicate` remains the sole
    /// authority for everything else. A protection-analysis failure returns
    /// `Err` so the caller fails safe to ALL-active — never a silent no-work.
    pub fn work_mask(&self) -> Result<WorkMask, String> {
        let cached = self.protections()?;
        let mut possible = self.hesitation.clone();
        for group in &self.repeats {
            for block in group.blocks() {
                for i in block {
                    possible[i] = true;
                }
            }
        }
        for (group, _replacement) in &self.restarts {
            for i in group.clone() {
                possible[i] = true;
            }
        }
        for (item, &hit) in possible.iter_mut().zip(&cached.hit) {
            if hit {
                *item = false;
            }
        }
        // Caps exposure walks the frozen sentence boundaries once (ordered
        // cursor, O(tokens + boundaries)): exposure is ON at index 0 and
        // re-armed at every boundary; it survives only while tokens are
        // deletable, so the FIRST non-deletable lowercase-start word after
        // an exposed filler is the legal cap target.
        let boundaries = sentence_boundaries(self.source);
        let mut cap_opportunity = vec![false; self.tokens.len()];
        let mut expose = true;
        let mut next = boundaries.iter().copied().filter(|&b| b > 0).peekable();
        for i in 0..self.tokens.len() {
            if next.peek() == Some(&i) {
                expose = true;
                next.next();
            }
            let value = &self.source[self.tokens[i].start..self.tokens[i].end];
            if expose
                && value
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
            {
                cap_opportunity[i] = true;
            }
            expose &= possible[i];
        }
        Ok(WorkMask {
            possible,
            cap_opportunity,
        })
    }
}

/// Adjudicate a KNOWN deletion-authority vector against `source` without any
/// proposal alignment. Flags align with `word_spans(source)`. This mirrors
/// the token-admissibility half of `validate_inner` for the case where the
/// caller already owns exact source-token deletions (an incremental
/// correction composing several independently-validated window authorities
/// into one whole-transcript edit), so no LCS/DP alignment or word-count cap
/// is needed or applied — the final text may legitimately exceed
/// `MAX_CLEANUP_WORDS`. Accepts iff every deleted token is a hesitation, or
/// a member of a fully-deleted repeat block whose group retains at least one
/// fully-kept sibling block, or a member of a fully-deleted restart group
/// whose replacement token is kept; no deleted token may overlap any
/// protection span (any kind, including term/capital Word protections — the
/// `locked` mask in `validate_inner` is an AND-not over all kinds); repeat
/// and restart groups are computed on the FULL text because cross-window
/// interactions are exactly what this gate exists to catch, and the
/// reconstructed text must preserve every Exact/Quote literal occurrence of
/// the source (the shared `LiteralGuard` check — a gap-character term whose
/// bytes belong to no token can still be erased OUTSIDE every deleted token,
/// which the token-overlap veto alone would miss; "Protected span changed").
/// All-false flags
/// return `source` unchanged; an all-true vector runs the same admissibility
/// rules as any other (see `DeletionContext::adjudicate`). Returns the
/// reconstructed candidate. Callers that need the full proposal-level
/// authority (ambiguity, comma-signature) must still pass the result through
/// `validate` — that comparison is proposal-relative and not reproducible
/// here.
pub fn validate_deletions(
    source: &str,
    deleted: &[bool],
    protected_terms: &[String],
) -> Result<String, String> {
    DeletionContext::prepare(source, protected_terms).adjudicate(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-local view of the public API: the frozen pass's output text.
    /// (The production `validate_cleanup` wrapper was removed with the
    /// bounded planner; tests observe `validate_cleanup_with_edits`.)
    fn validate_cleanup(
        source: &str,
        proposal: &str,
        protected_terms: &[String],
    ) -> Result<String, String> {
        validate_cleanup_with_edits(source, proposal, protected_terms)
            .map(|edits| edits.output)
    }

    #[test]
    fn keeps_safe_deletions_without_adopting_rewrites_or_added_punctuation() {
        let source = "Uh I need the report. Please um keep the final instruction and number 859";
        let proposal = "I need the summary. Please keep the final instruction.";
        let cleaned = validate_cleanup(source, proposal, &[]).unwrap();
        assert_eq!(cleaned, "I need the report. Please keep the final instruction and number 859");
        assert!(validate(source, &cleaned, &[]).is_ok());
    }

    #[test]
    fn partial_cleanup_preserves_protected_mentions_quotes_and_vocabulary() {
        for (source, proposal, expected, terms) in [
            ("Uh keep the word um in this label.", "Keep the word in this label.", "keep the word um in this label.", vec![]),
            ("Um, Qwen said \"uh keep 859\" today.", "Qwen said \"keep 859\" today.", "Qwen said \"uh keep 859\" today.", vec![]),
            ("Uh keep um here and never omit 859.", "Keep here and omit 859.", "keep um here and never omit 859.", vec!["um".to_string()]),
        ] {
            let cleaned = validate_cleanup(source, proposal, &terms).unwrap();
            assert_eq!(cleaned, expected);
            assert!(validate(source, &cleaned, &terms).is_ok());
        }
    }

    #[test]
    fn unsupported_proposals_without_valid_deletions_remain_rejected() {
        for (source, proposal) in [
            ("Keep the final number 859.", "Keep the final number."),
            ("Never delete this.", "Delete this."),
            ("The literal word um is required.", "The literal word is required."),
            ("Keep the report.", "Write a summary."),
        ] {
            assert!(validate_cleanup(source, proposal, &[]).is_err());
        }
    }


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
    fn partial_results_stay_inside_the_original_edit_policy_across_frozen_cases() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/cleanup-validation-v6.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let proposal = case["proposal"].as_str().unwrap();
            let terms: Vec<String> = serde_json::from_value(case["protected_terms"].clone()).unwrap();
            if let Ok(output) = validate_cleanup(source, proposal, &terms) {
                assert_eq!(validate(source, &output, &terms).unwrap(), output);
            }
        }
        assert!(validate_cleanup(&"x".repeat(MAX_CLEANUP_BYTES + 1), "x", &[]).is_err());
        assert!(validate_cleanup(&"um ".repeat(MAX_CLEANUP_WORDS + 1), "um", &[]).is_err());
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

    /// The edit flags returned by `validate_cleanup_with_edits` are the
    /// frozen pass's own deletions: replaying them through
    /// `deletions_candidate` reproduces a source-valid reconstruction, and
    /// equals the delivered output whenever no capitalization substitution is
    /// involved. Every accepted fixture case must satisfy this.
    #[test]
    fn edit_flags_replay_the_frozen_deletions_across_the_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/cleanup-validation-v6.json"
        ))
        .unwrap();
        let mut exact = 0;
        let mut checked = 0;
        for case in fixture["cases"].as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let proposal = case["proposal"].as_str().unwrap();
            let terms: Vec<String> =
                serde_json::from_value(case["protected_terms"].clone()).unwrap();
            let Ok(edits) = validate_cleanup_with_edits(source, proposal, &terms) else {
                continue;
            };
            checked += 1;
            assert_eq!(edits.deleted.len(), word_spans(source).len(), "case {case}");
            let candidate = deletions_candidate(source, &edits.deleted)
                .expect("flags replay through frozen reconstruction");
            // Flags alone must yield a candidate the frozen pass accepts, and
            // its revalidated form must agree with the delivered output.
            let revalidated = validate(source, &candidate, &terms).unwrap();
            assert!(
                revalidated == edits.output || revalidated == candidate,
                "case {case}: revalidation diverged"
            );
            if revalidated == edits.output {
                exact += 1;
            }
        }
        assert!(checked > 50, "fixture must yield accepted cases");
        assert!(exact > 0, "at least one case replays without caps");
    }

    /// Driver contract (§4.4): a deletion subset confined to a target range
    /// can be spliced out of a full-window edit set and revalidated against
    /// the whole window; edits touching context are dropped by construction,
    /// and mismatched flag vectors fail closed.
    #[test]
    fn target_subset_revalidates_in_full_window_context() {
        let window = "As I said before, um we should um ship the report. Uh tomorrow, um yes.";
        let proposal = "As I said before we should ship the report. Tomorrow yes.";
        let edits = validate_cleanup_with_edits(window, proposal, &[]).unwrap();
        let spans = word_spans(window);
        // Target = second sentence *from its terminator boundary*: the driver
        // attaches a leading hesitation to its sentence (spec §4.3), so the
        // slice starts at "Uh".
        let target = spans
            .iter()
            .position(|s| &window[s.clone()] == "Uh")
            .unwrap();
        let subset: Vec<bool> = edits
            .deleted
            .iter()
            .enumerate()
            .map(|(i, &d)| d && target <= i && i < spans.len())
            .collect();
        let candidate = deletions_candidate(window, &subset).unwrap();
        // Target-side deletions (Uh, um) replay; the capital substitution is
        // NOT part of the flags (it lives in `edits.output` only — the driver
        // stores output strings, so this lowercase intermediate is expected).
        assert_eq!(
            candidate,
            "As I said before, um we should um ship the report. tomorrow, yes."
        );
        // Context-side deletions ("before, um", "we should um") stayed raw.
        assert!(candidate.contains("before, um we should um ship"));
        assert!(validate(window, &candidate, &[]).is_ok());
        assert!(deletions_candidate(window, &[true; 3]).is_none());
    }

    /// Observable protection behavior (no internal kind classification is
    /// asserted — only what an admitted/rejected deletion vector and the
    /// caps replay make visible): a multi-word term literal cannot be
    /// deleted even though both words are hesitations; quoted material —
    /// paired or unpaired-to-end-of-text — cannot be edited; and a sentence-
    /// initial Word-locked negation CAN ride a validator-authorized capital
    /// while an Exact locked capital (and anything in quotes) may not.
    #[test]
    fn protection_seams_observed_through_admission_and_caps() {
        // Term lock: "um um" as a literal term outlives the hesitation rule.
        let terms = vec!["um um".to_string()];
        let text = "say um um then go";
        let spans = word_spans(text);
        let mut flags = vec![false; spans.len()];
        flags[1] = true; // first "um" of the literal
        assert!(validate_deletions(text, &flags, &terms).is_err(), "term literal deleted");
        // Nothing else in the sentence is deletable; the all-false vector
        // (settled NO-CHANGE) remains admissible.
        let ok = vec![false; spans.len()];
        assert_eq!(validate_deletions(text, &ok, &terms).as_deref(), Ok(text));
        // Quoted interior: the filler inside quotes is untouchable.
        let quoted = "He said \"um uh stop\" now.";
        let qspans = word_spans(quoted);
        let mut qflags = vec![false; qspans.len()];
        for (i, s) in qspans.iter().enumerate() {
            if &quoted[s.clone()] == "um" {
                qflags[i] = true;
            }
        }
        assert!(validate_deletions(quoted, &qflags, &[]).is_err(), "quote interior edited");
        // Unpaired opener protects everything after it to end of text.
        let open = "The client said \"we um ship";
        let ospans = word_spans(open);
        let mut oflags = vec![false; ospans.len()];
        for (i, s) in ospans.iter().enumerate() {
            if &open[s.clone()] == "um" {
                oflags[i] = true;
            }
        }
        assert!(validate_deletions(open, &oflags, &[]).is_err(), "unpaired quote seam");
        // Caps replay: an Exact-locked capital and quoted words are immune —
        // a recorded flip letter reaching them is dropped fail-closed.
        let src = "um Dr Smith said \"go home\" now.";
        let sspans = word_spans(src);
        let mut sflags = vec![false; sspans.len()];
        sflags[0] = true; // delete "um"
        let base = validate_deletions(src, &sflags, &[]).expect("filler deletable");
        assert_eq!(base, "Dr Smith said \"go home\" now.");
        let bogus = vec![(2usize, 'G'), (4usize, 'H')]; // "Smith"→"G…", "go"→"H…"
        assert_eq!(authorized_caps(src, &sflags, &bogus, &[]), base, "flip letters must not rewrite");
    }

    /// A Word-locked negation at a sentence start accepts the capitalized
    /// shape exactly as the frozen pass does (end-to-end through the
    /// validator + caps replay), while quoted material never can.
    #[test]
    fn word_locked_negation_caps_at_sentence_start() {
        let source = "um never do that. keep this.";
        let proposal = "Never do that. keep this.";
        let edits = validate_cleanup_with_edits(source, proposal, &[]).expect("cap admitted");
        assert_eq!(edits.output, proposal);
        assert_eq!(edits.deleted, vec![true, false, false, false, false, false]);
        let deletions = validate_deletions(source, &edits.deleted, &[]).expect("vector stands");
        assert_eq!(deletions, "never do that. keep this.");
        let flips = caps_flips(source, &edits.output, &edits.deleted).expect("caps extractable");
        assert_eq!(authorized_caps(source, &edits.deleted, &flips, &[]), proposal);
    }

    #[test]
    fn no_op_and_shape_rejects_precede_protection_pattern_errors() {
        let term = "a".repeat(2_000_000);
        let terms = [term.clone()];
        let source = "um hello there ok";
        let flags = vec![false; word_spans(source).len()];
        assert_eq!(validate_deletions(source, &flags, &terms).as_deref(), Ok(source));
        assert!(validate_deletions(source, &vec![true; flags.len()], &terms).is_err());
        let mut broken = flags.clone();
        broken.push(false);
        assert!(validate_deletions(source, &broken, &terms).is_err());
        let mut one = flags;
        one[0] = true;
        assert!(validate_deletions(source, &one, &terms).is_err());
    }

    /// A repeat run broken by an interior sentence break is rejected as a
    /// whole, and rejecting it must not shadow the first gap-ordinary run
    /// behind it: the second copy of the trailing pair stays deletable, and
    /// reconstruct trims the punctuation stranded after the kept copy.
    #[test]
    fn bad_gap_maximal_run_does_not_shadow_ordinary_suffix_run() {
        let source = "the the.\nthe the";
        let flags = [false, false, false, true];
        let out = validate_deletions(source, &flags, &[]).expect("suffix run authorized");
        assert_eq!(out, "the the.\nthe");
        // Deleting both trailing copies empties the only authorized group —
        // the frozen per-block kept-sibling rule rejects it.
        let both = [false, false, true, true];
        assert!(validate_deletions(source, &both, &[]).is_err());
    }

    /// The frozen validator requires every Exact/Quote literal occurrence in
    /// the source to survive a proposal ("Protected span changed" — a gap
    /// character like "." is an Exact term, so the empty text a fully-deleted
    /// filler source reconstructs to is NOT accepted). The adjudicator's
    /// token-overlap veto is blind to such bytes (they belong to no token),
    /// so the shared literal guard must reject the same candidate the frozen
    /// helper rejects.
    #[test]
    fn adjudicated_deletions_preserve_gap_term_literals() {
        let source = "um .";
        let terms = [".".to_string()];
        let flags = [true];
        assert_eq!(
            validate_deletions(source, &flags, &terms).unwrap_err(),
            "Protected span changed"
        );
        // Same verdict from the frozen whole-proposal gate.
        assert!(validate(source, "", &terms).is_err());
        // Control: with no term protecting the period, emptying an all-
        // hesitation source is the frozen legacy contract — it ACCEPTS, and
        // the guard must not change that.
        assert_eq!(validate_deletions(source, &flags, &[]).as_deref(), Ok(""));
        // Control: the comma attached to a word forms NO span (the term
        // pattern is word-delimited), so nothing is required and the
        // filler-only deletion stands.
        let keep = "keep um,";
        assert_eq!(
            validate_deletions(keep, &[false, true], &[",".to_string()]).as_deref(),
            Ok("keep")
        );
        assert!(validate(keep, "keep", &[",".to_string()]).is_ok());
    }

    /// `authorized_caps` vetoes flips that OVERLAP a protection span, but a
    /// far standalone Exact term makes its literal required in EVERY
    /// position, including inside an untouched longer word: with term "in"
    /// the source's two occurrences are the prefix of "inside" and the
    /// terminal word, and capitalising "inside" at a sentence start would
    /// drop the count to one. The shared literal guard reverts exactly that
    /// flip — the deletion and every literal-safe cap still render, and the
    /// output passes the frozen terminal gate.
    #[test]
    fn caps_flips_revert_only_literal_count_losses() {
        let source = "um inside the warehouse. the roof beams rest on oak posts and steel clamps keep them steady while workers calibrate torque twice daily panels are measured in.";
        let terms = ["in".to_string()];
        let mut deleted = vec![false; word_spans(source).len()];
        deleted[0] = true; // delete "um"
        let out = authorized_caps(source, &deleted, &[(1, 'I')], &terms);
        assert!(out.starts_with("inside the warehouse."), "unsafe cap leaked: {out}");
        assert!(validate(source, &out, &terms).is_ok(), "rendered text must pass the frozen gate");
        // Token 4 — the "the" sentence-initial after "warehouse. " — is a
        // REAL valid cap unaffected by any count rule, and must be RETAINED
        // alongside the reverted flip.
        let both = authorized_caps(source, &deleted, &[(1, 'I'), (4, 'T')], &terms);
        assert!(both.starts_with("inside the warehouse."), "unsafe cap leaked: {both}");
        assert!(both.contains(". The roof "), "safe cap dropped: {both}");
        assert!(validate(source, &both, &terms).is_ok());
    }
}
