"""Experimental source-deletion validator; no semantic safety guarantee.

Version 6 revises reviewed eligibility/protection rules on revealed development data. No model
loading, production imports, network, or release-qualification labels. Run this file directly for focused unit checks.
"""
from dataclasses import dataclass
import re
import unittest

MAX_BYTES = 32_000
MAX_WORDS = 1_024
MAX_ALIGNMENT_STATES = 20_000
MAX_ALIGNMENTS = 128
HESITATIONS = frozenset({'um', 'uh', 'erm', 'er', 'uhm', 'umm', 'hmm'})
ARTICLES = frozenset({'a', 'an', 'the'})
DETERMINERS = ARTICLES | {'this', 'that', 'these', 'those'}
NEGATIONS = frozenset({'no', 'not', 'never', 'neither', 'nor', 'without'})
FUNCTION_WORDS = frozenset({
    'a', 'an', 'the', 'i', 'we', 'you', 'he', 'she', 'it', 'they',
    'to', 'of', 'for', 'from', 'in', 'on', 'at', 'with', 'by', 'and', 'or',
    'is', 'are', 'was', 'were', 'have', 'has', 'had', 'this', 'that',
})
SINGLE_REPEAT_PROTECTED = frozenset({
    'very', 'really', 'so', 'too', 'much', 'more', 'less', 'quite', 'rather',
    'extremely', 'absolutely', 'yes', 'yeah', 'yep', 'yup', 'nope', 'okay', 'ok',
    'right', 'hey', 'wait', 'stop', 'please',
})
HONORIFICS = frozenset({'dr', 'mr', 'mrs', 'ms', 'mx', 'prof', 'rev', 'hon'})
LITERAL_NOUNS = frozenset({'string', 'literal', 'label', 'labels', 'tag', 'tags',
                           'parameter', 'parameters'})
MENTION_MARKERS = frozenset({
    'word', 'words', 'token', 'tokens', 'name', 'names',
    'letter', 'letters', 'identifier', 'identifiers', 'sequence', 'spell', 'type',
})
UNITS = frozenset({'mm', 'cm', 'm', 'km', 'kg', 'g', 'mg', 'lb', 'lbs',
                   'ms', 's', 'seconds', 'minutes', 'hours', 'inches', 'feet',
                   'percent', 'dollars', 'volts', 'amps', 'watts'})
WORD = re.compile(r"\w+(?:['’]\w+)*", re.UNICODE)
IDENTIFIER = re.compile(
    r'https?://[^\s]+|www\.[^\s]+|[^\s@]+@[^\s@]+|'
    r'\b\w+(?:[._:/+\-]\w+)+\b|\b\w*[\d_]\w*\b', re.UNICODE
)


@dataclass(frozen=True)
class Token:
    value: str
    start: int
    end: int
    byte_start: int
    byte_end: int

    @property
    def key(self):
        return self.value.casefold()


@dataclass(frozen=True)
class Result:
    accepted: bool
    output: str
    reason: str
    deletion_bytes: tuple = ()
    categories: tuple = ()
    equivalent_alignments: int = 0
    capitalization_bytes: tuple = ()
    gap_insertions: tuple = ()
    semantic_guarantee: bool = False


def tokenize(text):
    byte_offsets = [0]
    for char in text:
        byte_offsets.append(byte_offsets[-1] + len(char.encode('utf-8')))
    return [Token(m.group(), m.start(), m.end(),
                  byte_offsets[m.start()], byte_offsets[m.end()])
            for m in WORD.finditer(text)]


def _sentence_start(text, tokens, index):
    if index == 0:
        return True
    gap = text[tokens[index-1].end:tokens[index].start]
    return '\n' in gap or bool(re.fullmatch(r'[.!?]+[ \t]*', gap))


def _word_char(char):
    return bool(char) and (char.isalnum() or char == '_')


def _escaped_at(text, position):
    start = position
    while start and text[start-1] == '\\':
        start -= 1
    return (position-start) % 2 == 1


def _quote_code_spans(text):
    """Conservative protected spans, not a Markdown renderer.

    A complete backtick run closes only with the same complete, unescaped run.
    Shorter, longer and escaped runs remain code payload. Quotes preserve v4's
    backslash escaping and in-word apostrophes. Unterminated spans protect EOF.
    Escaped openers outside a span still conservatively begin protection.
    """
    spans, i = [], 0
    while i < len(text):
        start, char = i, text[i]
        if char == '`':
            while i < len(text) and text[i] == '`':
                i += 1
            width, end = i-start, len(text)
            while i < len(text):
                if text[i] != '`':
                    i += 1
                    continue
                run = i
                while i < len(text) and text[i] == '`':
                    i += 1
                if i-run == width and not _escaped_at(text, run):
                    end = i
                    break
            spans.append((start, end))
            i = end
            continue
        close = {'"': '"', '“': '”', '‘': '’', "'": "'"}.get(char)
        if close is None or (char == "'" and start and _word_char(text[start-1])):
            i += 1
            continue
        i += 1
        end = len(text)
        while i < len(text):
            if text[i] == '\\':
                i = min(i+2, len(text))
                continue
            inside_word = (close in {"'", '’'} and i and i+1 < len(text)
                           and _word_char(text[i-1]) and _word_char(text[i+1]))
            if text[i] == close and not inside_word:
                end = i+1
                break
            i += 1
        spans.append((start, end))
        i = end
    return spans


def _canonical_quotes(text):
    """Normalize paired smart/straight delimiter styles, never inner payload/code."""
    parts, cursor = [], 0
    for start, end in _quote_code_spans(text):
        parts.append(text[cursor:start])
        value = text[start:end]
        for opening, closing, canonical in [('“', '”', '"'), ('‘', '’', "'")]:
            if (value.startswith(opening) and value.endswith(closing)
                    and len(value) >= 2 and not _escaped_at(value, len(value)-1)):
                value = canonical + value[1:-1] + canonical
                break
        parts.append(value)
        cursor = end
    parts.append(text[cursor:])
    return ''.join(parts)


def _literal_context_spans(text, tokens):
    """Bounded explicit payload syntax; this does not infer speaker intention.

    Unlike the inherited whole-clause word/name guard, these lock only a small
    immediately named payload. Ordinary fillers later in the clause remain
    eligible. The notation rule requires an explicit definition verb, and the
    reported-sound rule requires both a reporting verb and an exact-sound cue.
    """
    spans = []
    keys = [token.key for token in tokens]
    for i, token in enumerate(tokens):
        if token.key in LITERAL_NOUNS:
            start = i + 1
            if start < len(tokens) and keys[start] in {'literal', 'value'}:
                start += 1
            elif (token.key in {'label', 'labels'} and start < len(tokens)
                  and keys[start] in {'reads', 'says', 'is', 'was'}):
                start += 1
            elif (start+1 < len(tokens) and keys[start] in {'starts', 'ends'}
                  and keys[start+1] in {'in', 'with'}):
                start += 2
            if start < len(tokens) and re.fullmatch(r'[ \t:]*', text[token.end:tokens[start].start]) is None:
                # Permit only the explicitly recognized bounded introducer.
                if start == i+1 or not _ordinary_gap(text, tokens, i, start):
                    continue
            if start < len(tokens):
                end = start + 1
                while end < min(start+4, len(tokens)) and keys[end] == keys[start]:
                    end += 1
                if _ordinary_gap(text, tokens, start, end-1):
                    spans.append((tokens[start].start, tokens[end-1].end, 'explicit literal payload'))
        if token.key == 'notation':
            for end in range(i+2, min(i+6, len(tokens))):
                if keys[end] in {'means', 'denotes', 'represents'}:
                    if re.fullmatch(r'[ \t,:]*', text[token.end:tokens[i+1].start]) and _ordinary_gap(text, tokens, i+1, end):
                        spans.append((tokens[i+1].start, tokens[end-1].end, 'explicit notation payload'))
                    break
        if token.key in {'said', 'answered', 'replied', 'uttered', 'responded'}:
            start = i+1
            end = start
            while end < min(start+4, len(tokens)) and keys[end] in HESITATIONS:
                end += 1
            if end == start or not _ordinary_gap(text, tokens, i, end-1):
                continue
            window_end = min(end+12, len(tokens))
            boundary = next((j for j in range(end, window_end)
                             if re.search(r'[.!?\n]', text[tokens[j-1].end:tokens[j].start])), window_end)
            cues = set(keys[end:boundary])
            if cues & {'sound', 'utterance', 'syllable'} and cues & {'exact', 'literal', 'recorded', 'transcribed'}:
                spans.append((tokens[start].start, tokens[end-1].end, 'explicit reported sound'))
    return spans


def protected_spans(text, tokens, dictionary, repeats):
    spans = [(a, b, 'quote/code') for a, b in _quote_code_spans(text)]
    spans.extend((m.start(), m.end(), 'identifier/number') for m in IDENTIFIER.finditer(text))
    for term in dictionary:
        if not isinstance(term, str) or not term:
            continue
        spans.extend((m.start(), m.end(), 'dictionary') for m in
                     re.finditer(r'(?<!\w)' + re.escape(term) + r'(?!\w)', text, re.IGNORECASE))
    for i, token in enumerate(tokens):
        if token.key in NEGATIONS or token.key.endswith(("n't", 'n’t')):
            spans.append((token.start, token.end, 'negation'))
        if token.key in UNITS and i and any(c.isdigit() for c in tokens[i-1].value):
            spans.append((token.start, token.end, 'unit'))
        if i and token.value[:1].isupper():
            previous = tokens[i-1]
            initial = (len(previous.value) == 1 and previous.value.isalpha()
                       and previous.value.isupper())
            if ((previous.key in HONORIFICS or initial)
                    and re.fullmatch(r'\.[ \t]*', text[previous.end:token.start])):
                spans.append((token.start, token.end, 'honorific/initial name'))
        initial_common_restart = (
            _sentence_start(text, tokens, i)
            and any(len(block) > 1 and block[0] == i
                    and any(tokens[other[0]].value == token.value.lower()
                            for other in group if other != block)
                    for group in repeats for block in group)
        )
        if (token.value[:1].isupper() and token.key not in FUNCTION_WORDS
                and not (_sentence_start(text, tokens, i) and token.key in HESITATIONS)
                and not initial_common_restart):
            spans.append((token.start, token.end, 'capitalized word'))
        # A conservative, explicit mention form; not general language understanding.
        if token.key in MENTION_MARKERS:
            end = re.search(r'[.!?\n]', text[token.end:])
            boundary = token.end + end.start() if end else len(text)
            spans.append((token.start, boundary, 'explicit word/name mention'))
    spans.extend(_literal_context_spans(text, tokens))
    return spans


def _ordinary_gap(text, tokens, first, last):
    return all(re.fullmatch(r'[ \t,]*', text[tokens[i].end:tokens[i+1].start])
               for i in range(first, last))


def _repeat_groups(text, tokens):
    """Bounded 1..4-token adjacent patterns; repetitions remain contextual."""
    groups = []
    keys = [t.key for t in tokens]
    for size in range(1, 5):
        for start in range(len(tokens) - 2*size + 1):
            pattern = keys[start:start+size]
            # This exclusion applies only to isolated repeated words. It must
            # not lock words inside otherwise eligible multiword restarts.
            if size == 1 and pattern[0] in SINGLE_REPEAT_PROTECTED:
                continue
            if size > 1 and len(set(pattern)) == 1:
                continue
            if pattern != keys[start+size:start+2*size]:
                continue
            end = start+2*size
            while keys[end:end+size] == pattern:
                end += size
            if _ordinary_gap(text, tokens, start, end-1):
                groups.append(tuple(tuple(range(i, i+size)) for i in range(start, end, size)))
    return groups


def _restart_groups(text, tokens):
    """Article + 1..4 hesitations + optional 'yeah,' + replacement determiner.

    'yeah' is eligible only inside this bounded pattern after at least two
    hesitations, with a comma before the retained replacement determiner.
    This syntax cannot establish that every unquoted 'yeah' is dispensable.
    """
    groups = []
    for i, token in enumerate(tokens):
        if token.key not in ARTICLES:
            continue
        j = i+1
        while j < len(tokens) and tokens[j].key in HESITATIONS and j-i <= 4:
            j += 1
        count = j-i-1
        if not 1 <= count <= 4:
            continue
        if j < len(tokens) and tokens[j].key == 'yeah':
            if count < 2 or j+1 >= len(tokens):
                continue
            if ',' not in text[tokens[j].end:tokens[j+1].start]:
                continue
            j += 1
        if j < len(tokens) and tokens[j].key in DETERMINERS and _ordinary_gap(text, tokens, i, j):
            groups.append((frozenset(range(i, j)), j))
    return groups


def _merge(spans):
    result = []
    for start, end in sorted(spans):
        if result and start <= result[-1][1]:
            result[-1] = (result[-1][0], max(end, result[-1][1]))
        else:
            result.append((start, end))
    return result


def _paired_filler_dashes(text, tokens, deleted):
    """Only paired em-dashes surrounding 1..4 deleted hesitations, between words."""
    result = []
    for match in re.finditer(r'—([^—\n]*)—', text):
        inside = [i for i, token in enumerate(tokens)
                  if token.start >= match.start(1) and token.end <= match.end(1)]
        if not 1 <= len(inside) <= 4 or not set(inside) <= deleted:
            continue
        if any(tokens[i].key not in HESITATIONS for i in inside):
            continue
        if not re.fullmatch(r'[ \t,]*', WORD.sub('', match.group(1))):
            continue
        left, right = inside[0]-1, inside[-1]+1
        if left < 0 or right >= len(tokens) or left in deleted or right in deleted:
            continue
        start, end = match.start(), match.end()
        while start and text[start-1] in ' \t':
            start -= 1
        while end < len(text) and text[end] in ' \t':
            end += 1
        if start != tokens[left].end or end != tokens[right].start:
            continue
        result.append((start, end, left, right))
    return result


def _reconstruct(text, tokens, deleted):
    if len(deleted) == len(tokens):
        if re.fullmatch(r'[\w\s,.]*', text) and '\n' not in text:
            return '', ((0, len(text.encode('utf-8'))),), (), []
        return None, (), (), []
    pairs = _paired_filler_dashes(text, tokens, deleted)
    spans = [(a, b) for a, b, _, _ in pairs]
    for i in sorted(deleted):
        start, end = tokens[i].start, tokens[i].end
        if any(a <= start and end <= b for a, b, _, _ in pairs):
            continue
        # Remove only a deleted token's attached comma and following horizontal gap.
        if end < len(text) and text[end] == ',':
            end += 1
        while end < len(text) and text[end] in ' \t':
            end += 1
        if all(j in deleted for j in range(i, len(tokens))):
            while start and text[start-1] in ' \t':
                start -= 1
            if start and text[start-1] == ',':
                start -= 1
        spans.append((start, end))
    merged = _merge(spans)
    pieces, position, gaps = [], 0, []
    for start, end in merged:
        pieces.append(text[position:start])
        if any(start <= a and b <= end for a, b, _, _ in pairs):
            pieces.append(' ')
            gaps.append(len(text[:start].encode('utf-8')))
        position = end
    pieces.append(text[position:])
    byte_spans = tuple((len(text[:a].encode('utf-8')), len(text[:b].encode('utf-8'))) for a, b in merged)
    return ''.join(pieces), byte_spans, tuple(gaps), pairs


def _proposal_local_gaps(proposal, proposed, kept, pairs):
    changes = []
    positions = {original: j for j, original in enumerate(kept)}
    for _, _, left, right in pairs:
        a, b = positions[left], positions[right]
        if b != a+1:
            return None
        start, end = proposed[a].end, proposed[b].start
        if not re.fullmatch(r'[ \t,]*(?:—[ \t,]*){0,2}', proposal[start:end]):
            return None
        changes.append((start, end))
    for start, end in reversed(changes):
        proposal = proposal[:start] + ' ' + proposal[end:]
    return proposal


def _punctuation_signature(text, *, keep_commas=False):
    # These choices never enter delivered text: comma/horizontal whitespace,
    # paired quote delimiter style, and one optional final period. No arbitrary
    # punctuation changes, question/exclamation changes, or newline flattening.
    canonical = _canonical_quotes(text)
    canonical = (re.sub(r'[ \t]*,[ \t]*', ',', canonical) if keep_commas
                 else canonical.replace(',', ' '))
    signature = re.sub(r'[ \t]+', ' ', canonical).strip(' \t')
    if signature.endswith('.') and not signature.endswith('..'):
        signature = signature[:-1]
    return signature


def validate(source, proposal, *, dictionary=(), completed=True):
    def reject(reason):
        return Result(False, source, reason)

    if not isinstance(source, str) or not isinstance(proposal, str):
        return reject('Source and proposal must be text')
    if not completed:
        return reject('Generation did not complete')
    try:
        if max(len(source.encode('utf-8')), len(proposal.encode('utf-8'))) > MAX_BYTES:
            return reject('Input/output byte limit exceeded')
        tokens, proposed = tokenize(source), tokenize(proposal)
    except UnicodeEncodeError:
        return reject('Malformed Unicode')
    if source == proposal:
        return Result(True, source, 'No change', equivalent_alignments=1)
    if not tokens or len(tokens) > MAX_WORDS or len(proposed) > len(tokens):
        return reject('Word limit or word addition')
    repeats = _repeat_groups(source, tokens)
    protected = protected_spans(source, tokens, dictionary, repeats)
    locked = {i for i, t in enumerate(tokens) if any(t.start < b and t.end > a for a, b, _ in protected)}
    # Exact syntactic/dictionary spans include punctuation as well as words.
    for a, b, reason in protected:
        if reason in {'quote/code', 'identifier/number', 'dictionary'}:
            value = source[a:b]
            source_check, proposal_check = source, proposal
            if reason == 'quote/code':
                value = _canonical_quotes(value)
                source_check, proposal_check = _canonical_quotes(source), _canonical_quotes(proposal)
            if proposal_check.count(value) < source_check.count(value):
                return reject('Protected span changed: ' + reason)
    restarts = _restart_groups(source, tokens)
    possible = {i for i, t in enumerate(tokens) if t.key in HESITATIONS}
    possible.update(i for group in repeats for block in group for i in block)
    possible.update(i for group, _ in restarts for i in group)
    possible -= locked
    # Explicit stack, bounded search, no recursive depth proportional to dictation.
    stack = [(0, 0, ())]
    alignments, states = [], 0
    while stack:
        i, j, kept = stack.pop()
        states += 1
        if states > MAX_ALIGNMENT_STATES:
            return reject('Alignment work limit exceeded')
        if len(tokens)-i < len(proposed)-j:
            continue
        if i == len(tokens):
            if j == len(proposed):
                if len(alignments) >= MAX_ALIGNMENTS:
                    return reject('Alignment count limit exceeded')
                alignments.append(kept)
            continue
        if i in possible:
            stack.append((i+1, j, kept))
        if j < len(proposed) and tokens[i].key == proposed[j].key:
            same_case = tokens[i].value == proposed[j].value
            initial_cap = (tokens[i].value[:1] in 'abcdefghijklmnopqrstuvwxyz'
                           and proposed[j].value == tokens[i].value[:1].upper()+tokens[i].value[1:])
            if same_case or initial_cap:
                stack.append((i+1, j+1, kept+(i,)))
    accepted = {}
    for kept in alignments:
        deleted = set(range(len(tokens))) - set(kept)
        allowed = {i for i in deleted if tokens[i].key in HESITATIONS}
        categories = {'hesitation'} if allowed else set()
        for group in repeats:
            if any(all(i not in deleted for i in block) for block in group):
                removable = {i for block in group if all(j in deleted for j in block) for i in block}
                if removable:
                    allowed.update(removable)
                    categories.add('adjacent repeated span')
        for group, replacement in restarts:
            if group <= deleted and replacement not in deleted:
                allowed.update(group)
                categories.add('bounded article restart')
        if deleted != allowed:
            continue
        output, spans, gaps, pairs = _reconstruct(source, tokens, deleted)
        if output is None:
            continue
        output_tokens = tokenize(output)
        caps = []
        invalid_case = False
        for j, i in enumerate(kept):
            if tokens[i].value == proposed[j].value:
                continue
            if not _sentence_start(output, output_tokens, j):
                invalid_case = True
                break
            caps.append((tokens[i].byte_start, tokens[i].value[0], proposed[j].value[0]))
        if invalid_case:
            continue
        for j in reversed(range(len(kept))):
            if tokens[kept[j]].value != proposed[j].value:
                position = output_tokens[j].start
                output = output[:position] + proposed[j].value[0] + output[position+1:]
        if caps:
            categories.add('sentence-initial capitalization')
        comparison = _proposal_local_gaps(proposal, proposed, kept, pairs)
        if comparison is None or _punctuation_signature(output) != _punctuation_signature(comparison):
            continue
        if gaps:
            categories.add('paired filler em-dash gap')
        comma_match = (_punctuation_signature(output, keep_commas=True)
                       == _punctuation_signature(comparison, keep_commas=True))
        if output in accepted:
            accepted[output][2] += 1
            accepted[output][5] |= comma_match
        else:
            accepted[output] = [spans, tuple(sorted(categories)), 1, tuple(caps), gaps, comma_match]
    if not accepted:
        return reject('Proposal requires protected/unsupported edits or punctuation changes')
    if len(accepted) != 1:
        # Choose only an already-valid source reconstruction. Comparison never
        # supplies delivered punctuation, whitespace or words.
        matching = {output: data for output, data in accepted.items() if data[5]}
        if len(matching) != 1:
            return reject('Ambiguous source reconstruction')
        accepted = matching
    output, (spans, categories, count, caps, gaps, _) = next(iter(accepted.items()))
    return Result(True, output, 'Source-derived deletions accepted', spans, categories, count, caps, gaps)


class ValidatorTests(unittest.TestCase):
    def assert_accept(self, source, proposal, expected=None, **kwargs):
        result = validate(source, proposal, **kwargs)
        self.assertTrue(result.accepted, result)
        self.assertEqual(result.output, proposal if expected is None else expected)
        return result

    def assert_reject(self, source, proposal, **kwargs):
        result = validate(source, proposal, **kwargs)
        self.assertFalse(result.accepted, result)
        self.assertEqual(result.output, source)

    def test_adjacent_duplicates_have_equivalent_alignments(self):
        result = self.assert_accept('I I need the the report.', 'I need the report.')
        self.assertGreater(result.equivalent_alignments, 1)

    def test_repeated_phrase(self):
        self.assert_accept('We need we need the report.', 'We need the report.')
        self.assert_accept('Please attach please attach the photo.', 'Please attach the photo.')
        self.assert_accept('Send it send it tomorrow.', 'Send it tomorrow.')
        self.assert_reject('Riley left Riley left today.', 'Riley left today.')
        self.assert_reject('very very very very good', 'very very good')

    def test_multi_filler_article_restart_general_category(self):
        self.assert_accept('Keep all the um uh yeah, those things.', 'Keep all those things.')
        self.assert_accept('Use a uh er yeah, this folder.', 'Use this folder.')
        self.assert_accept('Send an um the invoice.', 'Send the invoice.')

    def test_capitalized_leading_and_terminal_fillers(self):
        self.assert_accept('Um, we can go, uh.', 'We can go.')
        self.assert_accept('We can go uh', 'We can go')
        self.assert_accept('um uh', '')
        result = self.assert_accept('All set. Um, bring it.', 'All set. Bring it.')
        self.assertTrue(result.capitalization_bytes)
        self.assert_accept('All set.\n\nUm, bring it.', 'All set.\n\nBring it.')
        self.assert_reject('All set. Um, bring it.', 'All set. Bring It.')
        self.assert_reject('All set. Um, bring it.', 'All set. Bring it.', dictionary=['Um'])

    def test_utf8_offsets_reconstruct_source(self):
        for source, proposal in [
            ('José um needs café tomorrow.', 'José needs café tomorrow.'),
            ('José is here. Um, bring café.', 'José is here. Bring café.'),
            ('José—um—needs café.', 'José needs café.'),
        ]:
            result = self.assert_accept(source, proposal)
            data = source.encode('utf-8')
            for offset, before, after in result.capitalization_bytes:
                self.assertEqual(data[offset:offset+1].decode('ascii'), before)
                data = data[:offset] + after.encode('ascii') + data[offset+1:]
            for start, end in reversed(result.deletion_bytes):
                data = data[:start] + (b' ' if start in result.gap_insertions else b'') + data[end:]
            self.assertEqual(data.decode('utf-8'), result.output)

    def test_quotes_code_dictionary_and_identifiers(self):
        for source, proposal in [
            ('Print "um um" here.', 'Print "um" here.'),
            ('Keep `the the` intact.', 'Keep `the` intact.'),
            ('Use Qwen3.8-Flash-Next um today.', 'Use Qwen3.8-Flash um today.'),
            ('The value is 1,500 um dollars.', 'The value is 1500 dollars.'),
            ('Run --no-cache um now.', 'Run --cache now.'),
        ]:
            self.assert_reject(source, proposal)
        self.assert_reject('Meet Um um tomorrow.', 'Meet tomorrow.', dictionary=['Um'])
        self.assert_reject('Use the the switch.', 'Use the switch.', dictionary=['the the'])

    def test_protected_content_and_final_clause(self):
        for source, proposal in [
            ('Do not not send it.', 'Do not send it.'),
            ('Pay 50 50 dollars.', 'Pay 50 dollars.'),
            ('Meet Anna Anna tomorrow.', 'Meet Anna tomorrow.'),
            ('We need the report by Friday.', 'We need the report.'),
            ('We should go.', 'We can go.'),
            ('We can go.', 'Can we go.'),
            ('We go.', 'We should go.'),
        ]:
            self.assert_reject(source, proposal)

    def test_literal_mentions_and_deliberate_emphasis(self):
        self.assert_reject('The token um should remain.', 'The token should remain.')
        self.assert_reject('Type a a into the box.', 'Type a into the box.')
        self.assert_reject('Repeat the words and and.', 'Repeat the words and.')
        self.assert_reject('This is very very good.', 'This is very good.')

    def test_unsupported_restart_and_agreement(self):
        self.assert_reject('Keep all the important things.', 'Keep all things.')
        self.assert_reject('Yes yeah those are mine.', 'Yes those are mine.')
        self.assert_reject('Use the um yeah, those folders.', 'Use those folders.')
        self.assert_reject('Use the um uh another folder.', 'Use another folder.')

    def test_source_punctuation_and_paragraphs_are_authoritative(self):
        self.assert_accept('I came, um, this morning.', 'I came this morning.', 'I came, this morning.')
        self.assert_reject('We go? Um, tomorrow.', 'We go. Tomorrow.')
        self.assert_reject('We go.\n\nUm, tomorrow.', 'We go. Tomorrow.')
        self.assert_accept('We go.\n\nUm, tomorrow.', 'We go.\n\nUm, tomorrow.')

    def test_source_comma_tie_break_requires_one_existing_reconstruction(self):
        self.assert_accept('I, I need the report.', 'I need the report.')
        self.assert_accept('At the moment at the moment, the server is unavailable.',
                           'At the moment, the server is unavailable.')
        self.assert_accept('Before we start before we start, check the room.',
                           'Before we start, check the room.')
        self.assert_reject('I I  need the report.', 'I need the report.')
        self.assert_reject('I, I need the report.', 'I; need the report.')

    def test_incomplete_and_work_limits(self):
        self.assert_reject('We um need it.', 'We need it.', completed=False)
        self.assert_reject('the ' * 200, 'the ' * 100)
        self.assert_reject('x' * (MAX_BYTES+1), 'x')

    def test_known_contextual_limits_are_explicit(self):
        # These are counterexamples, not acceptable semantics or release passes.
        # An unquoted word can be meaningful despite matching allowed syntax.
        foreign = self.assert_accept('Há um sensor.', 'Há sensor.')
        emphasis = self.assert_accept('We we are responsible.', 'We are responsible.')
        self.assertFalse(foreign.semantic_guarantee)
        self.assertFalse(emphasis.semantic_guarantee)

    def test_bounded_literal_payloads(self):
        self.assert_reject('Enter the string er into that field.', 'Enter the string into that field.')
        self.assert_reject('Preserve the literal uh for comparison.', 'Preserve the literal for comparison.')
        self.assert_reject('Use the string value um um for the test.', 'Use the string value um for the test.')
        self.assert_accept('The string is um tangled.', 'The string is tangled.')
        self.assert_accept('The string red is um fine.', 'The string red is fine.')
        self.assert_accept('The string breaks. Um, check it.', 'The string breaks. Check it.')

    def test_bounded_notation_payloads(self):
        self.assert_reject('In our notation, a a denotes two distinct inputs.',
                           'In our notation, a denotes two distinct inputs.')
        self.assert_reject('The notation: to to represents a pair.', 'The notation: to represents a pair.')
        self.assert_accept('This notation is um hard to read.', 'This notation is hard to read.')
        self.assert_accept('In our notation, x means um multiply.', 'In our notation, x means multiply.')

    def test_explicit_reported_sound_is_distinct_from_an_ordinary_filler(self):
        self.assert_reject('She replied erm, and the clerk transcribed that exact syllable.',
                           'She replied and the clerk transcribed that exact syllable.')
        self.assert_reject('He uttered uh uh, which was recorded as a literal sound.',
                           'He uttered uh, which was recorded as a literal sound.')
        self.assert_accept('She answered um the next question carefully.',
                           'She answered the next question carefully.')
        self.assert_accept('She said uh send the exact form.', 'She said send the exact form.')
        self.assert_accept('She said um send it. Record that exact sound later.',
                           'She said send it. Record that exact sound later.')

    def test_final_period_is_source_owned(self):
        self.assert_accept('We arrive Monday erm', 'We arrive Monday.', 'We arrive Monday')
        self.assert_accept('We um arrive Monday.', 'We arrive Monday', 'We arrive Monday.')
        self.assert_reject('We um arrive Monday?', 'We arrive Monday.')
        self.assert_reject('We um arrive Monday!', 'We arrive Monday.')
        self.assert_reject('We um arrive Monday...', 'We arrive Monday.')

    def test_paired_filler_dashes_have_one_explicit_gap(self):
        for source in ['Ask Léa—uh—to bring coffee.', 'Ask Léa — uh, erm — to bring coffee.']:
            for proposal in ['Ask Léa to bring coffee.', 'Ask Léa—to bring coffee.', 'Ask Léa——to bring coffee.']:
                result = self.assert_accept(source, proposal, 'Ask Léa to bring coffee.')
                self.assertEqual(len(result.gap_insertions), 1)
        self.assert_reject('Ask Léa—uh—to bring coffee.', 'Ask Léa; to bring coffee.')
        self.assert_reject('Ask Léa—uh to bring coffee.', 'Ask Léa to bring coffee.')
        self.assert_reject('Ask Léa—later—to bring coffee.', 'Ask Léa to bring coffee.')
        self.assert_reject('Ask Léa—uh—to bring coffee.', 'Ask Léa to bring coffee.', dictionary=['uh'])

    def test_quote_style_changes_preserve_exact_payload_and_original_delimiters(self):
        self.assert_accept('We we read ‘Keep this, please.’ yesterday.',
                           "We read 'Keep this, please.' yesterday.",
                           'We read ‘Keep this, please.’ yesterday.')
        self.assert_accept('We we read “Keep this.” yesterday.',
                           'We read "Keep this." yesterday.',
                           'We read “Keep this.” yesterday.')
        self.assert_reject('We we read ‘Keep this, please.’ yesterday.',
                           "We read 'Keep this please.' yesterday.")
        self.assert_reject('We we read `Keep this.` yesterday.', 'We read "Keep this." yesterday.')
        self.assert_reject('We we read ‘Keep this, please.’ yesterday.',
                           "We read 'Keep this, Please.' yesterday.")
        self.assert_accept('We we read ‘It isn’t um optional.’ yesterday.',
                           "We read 'It isn’t um optional.' yesterday.",
                           'We read ‘It isn’t um optional.’ yesterday.')
        self.assert_reject('We we read ‘It isn’t um optional.’ yesterday.',
                           "We read 'It isn’t optional.' yesterday.")
        self.assert_reject("We we read 'It isn't um optional.' yesterday.",
                           "We read 'It isn't optional.' yesterday.")

    def test_escaped_quotes_backslash_parity_and_unterminated_payloads(self):
        self.assert_reject(r'The exact text is "say \"um, uh\" now" on screen.',
                           r'The exact text is "say \"\" now" on screen.')
        self.assert_accept(r'We um show "say \"um, uh\" now" on screen.',
                           r'We show "say \"um, uh\" now" on screen.')
        self.assert_accept(r'We display "path\\" um today.',
                           r'We display "path\\" today.')
        self.assert_reject(r'We show "path\\\" uh inside" um today.',
                           r'We show "path\\\" inside" today.')
        self.assert_accept(r'We show "path\\\" uh inside" um today.',
                           r'We show "path\\\" uh inside" today.')
        self.assert_reject('We display "keep um' + '\\', 'We display "keep' + '\\')
        self.assert_reject('We show "keep\num unchanged" today.',
                           'We show "keep\nunchanged" today.')

    def test_escaped_contractions_and_quote_style_keep_the_inner_payload_exact(self):
        self.assert_reject(r"We show 'it\'s um intact' today.",
                           r"We show 'it\'s intact' today.")
        self.assert_accept("We um show 'it's uh intact'.", "We show 'it's uh intact'.")
        self.assert_accept(r'We um show “keep \”uh\” intact”.',
                           r'We show "keep \”uh\” intact".',
                           r'We show “keep \”uh\” intact”.')
        self.assert_reject(r'We um show “keep \”uh\” intact”.',
                           r'We show "keep \"uh\" intact".')

    def test_multiline_and_unterminated_quote_code_payloads(self):
        for opening, closing in [('"', '"'), ("'", "'"), ('“', '”'), ('‘', '’'), ('`', '`'), ('```', '```')]:
            with self.subTest(delimiters=(opening, closing)):
                self.assert_reject(f'Keep {opening}um,\nuh next{closing} verbatim.',
                                   f'Keep {opening}\nnext{closing} verbatim.')
                self.assert_reject(f'Keep {opening}um then\nnext{closing} verbatim.',
                                   f'Keep {opening}then\nnext{closing} verbatim.')
                self.assert_reject(f'Keep {opening}um then\nnext',
                                   f'Keep {opening}then\nnext')
                self.assert_accept(f'We um keep {opening}um,\nuh next{closing} verbatim.',
                                   f'We keep {opening}um,\nuh next{closing} verbatim.')

    def test_complete_backtick_runs_protect_nested_short_and_unterminated_payloads(self):
        for width in [1, 2, 3, 5, 12]:
            fence = '`' * width
            with self.subTest(width=width):
                self.assert_reject(f'Keep {fence}um uh{fence} intact.',
                                   f'Keep {fence}{fence} intact.')
                self.assert_accept(f'We um keep {fence}um uh{fence} intact.',
                                   f'We keep {fence}um uh{fence} intact.')
                self.assert_reject(f'Keep {fence}um uh', f'Keep {fence}')
                shorter = '`' * max(width-1, 1)
                if width > 1:
                    self.assert_reject(f'Keep {fence}before {shorter}um uh{shorter} after{fence} intact.',
                                       f'Keep {fence}before {shorter}{shorter} after{fence} intact.')
                longer = '`' * (width+1)
                self.assert_reject(f'Keep {fence}before {longer}um uh{longer} after{fence} intact.',
                                   f'Keep {fence}before {longer}{longer} after{fence} intact.')

    def test_code_escaped_delimiter_runs_remain_payload(self):
        self.assert_reject(r'Keep `say \`um uh\` now` intact.',
                           r'Keep `say \`\` now` intact.')
        self.assert_accept(r'We um keep ``say \``um uh\`` now`` intact.',
                           r'We keep ``say \``um uh\`` now`` intact.')
        self.assert_accept(r'Keep ``path\\`` um today.', r'Keep ``path\\`` today.')
        self.assert_reject(r'Keep ``path\`` um today.', r'Keep ``path\`` today.')

    def test_retained_negation_can_receive_narrow_initial_capitalization(self):
        self.assert_accept('Uh, not yet.', 'Not yet.')
        self.assert_accept("Um, don't send it.", "Don't send it.")
        self.assert_reject('Uh, not yet.', 'Yet.')
        self.assert_reject('Uh, "not yet" was written.', '"Not yet" was written.')
        self.assert_reject('Uh, not yet.', 'Not yet.', dictionary=['not yet'])

    def test_general_single_word_repeats_preserve_explicit_emphasis_and_names(self):
        for source, proposal in [
            ('We can can finish the test.', 'We can finish the test.'),
            ('She will will arrive soon.', 'She will arrive soon.'),
            ('Please bring bring the wrench.', 'Please bring the wrench.'),
            ('The device stopped stopped responding.', 'The device stopped responding.'),
            ('I opened opened the file.', 'I opened the file.'),
        ]:
            self.assert_accept(source, proposal)
        for word in ['very', 'really', 'so', 'too', 'yes', 'yeah', 'hey', 'wait', 'please', 'much']:
            self.assert_reject(f'We said {word} {word} today.', f'We said {word} today.')
        self.assert_reject('Meet Anna Anna tomorrow.', 'Meet Anna tomorrow.')
        self.assert_reject('Use qwen qwen now.', 'Use qwen now.', dictionary=['Qwen'])
        self.assert_accept('Please attach please attach the photo.', 'Please attach the photo.')
        self.assert_accept('It is very useful very useful here.', 'It is very useful here.')

    def test_bounded_labels_parameters_and_tag_endpoints(self):
        self.assert_accept('The return label should go on on the carton.',
                           'The return label should go on the carton.')
        for source, proposal in [
            ('The label reads um exactly.', 'The label reads exactly.'),
            ('The label says uh clearly.', 'The label says clearly.'),
            ('The label is um.', 'The label is.'),
            ('The label was er.', 'The label was.'),
            ('Set the parameter um to zero.', 'Set the parameter to zero.'),
            ('The tag ends in uh, um.', 'The tag ends in.'),
            ('The tag starts with erm, uh.', 'The tag starts with.'),
        ]:
            self.assert_reject(source, proposal)
        self.assert_accept('The tag ends in uh, um.', 'The tag ends in uh.')
        self.assert_accept('The tag starts with erm, uh.', 'The tag starts with erm.')
        self.assert_accept('The parameter should um remain unchanged.',
                           'The parameter should remain unchanged.')

    def test_honorific_and_initial_periods_do_not_expose_names(self):
        for prefix in ['Dr.', 'Mr.', 'Ms.', 'Prof.', 'J.']:
            self.assert_reject(f'{prefix} Um um confirmed it.', f'{prefix} confirmed it.')
            self.assert_accept(f'{prefix} Um um confirmed it.', f'{prefix} Um confirmed it.')
        self.assert_accept('All set. Um, bring it.', 'All set. Bring it.')
        self.assert_accept('Go. Um, bring it.', 'Go. Bring it.')


if __name__ == '__main__':
    unittest.main(verbosity=2)
