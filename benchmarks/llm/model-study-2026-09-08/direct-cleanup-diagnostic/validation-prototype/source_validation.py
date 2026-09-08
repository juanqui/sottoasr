"""Experimental source-deletion validator; no semantic safety guarantee.

Only development examples belong here. No model loading, production imports,
network, or semantic60 labels. Run this file directly for focused unit checks.
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
MENTION_MARKERS = frozenset({
    'word', 'words', 'token', 'tokens', 'name', 'names', 'label', 'labels',
    'letter', 'letters', 'identifier', 'identifiers', 'sequence', 'spell', 'type',
})
UNITS = frozenset({'mm', 'cm', 'm', 'km', 'kg', 'g', 'mg', 'lb', 'lbs',
                   'ms', 's', 'seconds', 'minutes', 'hours', 'inches', 'feet',
                   'percent', 'dollars', 'volts', 'amps', 'watts'})
WORD = re.compile(r"\w+(?:['’]\w+)*", re.UNICODE)
QUOTES_CODE = re.compile(
    r'```[\s\S]*?(?:```|$)|`[^`\n]*(?:`|$)|"[^"\n]*(?:"|$)|'
    r'“[^”]*(?:”|$)|‘[^’]*(?:’|$)|(?<!\w)\x27[^\x27\n]*(?:\x27|$)'
)
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


def protected_spans(text, tokens, dictionary, repeats):
    spans = [(m.start(), m.end(), 'quote/code') for m in QUOTES_CODE.finditer(text)]
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
            # Preserve isolated emphatic content repeats such as 'very very'.
            if size == 1 and pattern[0] not in FUNCTION_WORDS:
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


def _reconstruct(text, tokens, deleted):
    if len(deleted) == len(tokens):
        if re.fullmatch(r'[\w\s,.]*', text) and '\n' not in text:
            return '', ((0, len(text.encode('utf-8'))),)
        return None, ()
    spans = []
    for i in sorted(deleted):
        start, end = tokens[i].start, tokens[i].end
        # Remove only a deleted token's attached comma and following horizontal gap.
        if end < len(text) and text[end] == ',':
            end += 1
        while end < len(text) and text[end] in ' \t':
            end += 1
        # A terminal filler must not leave a dangling preceding comma or space.
        if all(j in deleted for j in range(i, len(tokens))):
            while start and text[start-1] in ' \t':
                start -= 1
            if start and text[start-1] == ',':
                start -= 1
        spans.append((start, end))
    merged = _merge(spans)
    pieces, position = [], 0
    for start, end in merged:
        pieces.append(text[position:start])
        position = end
    pieces.append(text[position:])
    byte_spans = tuple((len(text[:a].encode('utf-8')), len(text[:b].encode('utf-8'))) for a, b in merged)
    return ''.join(pieces), byte_spans


def _punctuation_signature(text):
    # Model comma and horizontal whitespace choices never enter delivered text.
    # All other punctuation and every newline must match reconstruction.
    return re.sub(r'[ \t]+', ' ', text.replace(',', ' ')).strip(' \t')


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
            if proposal.count(value) < source.count(value):
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
            initial_cap = (i not in locked and tokens[i].value[:1] in 'abcdefghijklmnopqrstuvwxyz'
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
        output, spans = _reconstruct(source, tokens, deleted)
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
        if _punctuation_signature(output) != _punctuation_signature(proposal):
            continue
        if output in accepted:
            accepted[output][2] += 1
        else:
            accepted[output] = [spans, tuple(sorted(categories)), 1, tuple(caps)]
    if not accepted:
        return reject('Proposal requires protected/unsupported edits or punctuation changes')
    if len(accepted) != 1:
        return reject('Ambiguous source reconstruction')
    output, (spans, categories, count, caps) = next(iter(accepted.items()))
    return Result(True, output, 'Source-derived deletions accepted', spans, categories, count, caps)


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
        ]:
            result = self.assert_accept(source, proposal)
            data = source.encode('utf-8')
            for offset, before, after in result.capitalization_bytes:
                self.assertEqual(data[offset:offset+1].decode('ascii'), before)
                data = data[:offset] + after.encode('ascii') + data[offset+1:]
            for start, end in reversed(result.deletion_bytes):
                data = data[:start] + data[end:]
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

    def test_materially_different_duplicate_alignment_is_rejected(self):
        self.assert_reject('I, I need the report.', 'I need the report.')

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


if __name__ == '__main__':
    unittest.main(verbosity=2)
