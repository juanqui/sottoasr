import unittest
from unittest.mock import patch

import span_metrics as metrics


class SpanMetricsTests(unittest.TestCase):
    def test_complete_run_requires_all_fillers(self):
        partial = metrics.score_case('um uh send it', 'send it', 'uh send it')
        self.assertEqual(partial['correctly_deleted_tokens'], 1)
        self.assertEqual(partial['gold_delete_tokens'], 2)
        self.assertEqual(partial['gold_delete_runs'], 1)
        self.assertEqual(partial['completed_delete_runs'], 0)
        self.assertFalse(partial['complete_cleanup'])
        complete = metrics.score_case('um uh send it', 'send it', 'send it')
        self.assertEqual(complete['completed_delete_runs'], 1)
        self.assertTrue(complete['complete_cleanup'])

    def test_distinct_runs_are_counted_separately(self):
        result = metrics.score_case('um send uh it', 'send it', 'send uh it')
        self.assertEqual(result['gold_delete_runs'], 2)
        self.assertEqual(result['completed_delete_runs'], 1)

    def test_identical_repeat_copies_are_equivalent(self):
        result = metrics.score_case('I I I agree', 'I agree', 'I agree')
        self.assertEqual(result['correctly_deleted_tokens'], 2)
        self.assertEqual(result['gold_delete_runs'], 1)
        self.assertEqual(result['completed_delete_runs'], 1)
        self.assertTrue(result['preservation_valid'])
        partial = metrics.score_case('I I I agree', 'I agree', 'I I agree')
        self.assertEqual(partial['correctly_deleted_tokens'], 1)
        self.assertEqual(partial['gold_delete_runs'], 1)
        self.assertEqual(partial['completed_delete_runs'], 0)

    def test_repeated_phrase_is_one_complete_run(self):
        result = metrics.score_case('can we can we leave', 'can we leave', 'can we leave')
        self.assertEqual(result['correctly_deleted_tokens'], 2)
        self.assertEqual(result['completed_delete_runs'], 1)

    def test_wrong_literal_occurrence_gets_no_deletion_credit(self):
        result = metrics.score_case('um I said um', 'I said um', 'um I said')
        self.assertEqual(result['correctly_deleted_tokens'], 0)
        self.assertEqual(result['required_word_losses'], 1)
        self.assertFalse(result['preservation_valid'])

    def test_meaningful_loss_is_separate_from_removed_fillers(self):
        result = metrics.score_case('um keep not blue uh', 'keep not blue', 'keep blue')
        self.assertEqual(result['correctly_deleted_tokens'], 2)
        self.assertEqual(result['required_word_losses'], 1)
        self.assertEqual(result['safe_correctly_deleted_tokens'], 0)
        self.assertFalse(result['complete_cleanup'])

    def test_added_words_are_not_hidden_by_good_deletions(self):
        result = metrics.score_case('um keep blue', 'keep blue', 'keep blue today')
        self.assertEqual(result['correctly_deleted_tokens'], 1)
        self.assertEqual(result['source_word_additions'], 1)
        self.assertEqual(result['required_word_losses'], 0)
        self.assertEqual(result['safe_correctly_deleted_tokens'], 0)

    def test_reordering_fails_source_preservation(self):
        result = metrics.score_case('um alpha beta', 'alpha beta', 'beta alpha')
        self.assertFalse(result['preservation_valid'])
        self.assertEqual(result['source_word_additions'], 1)
        self.assertEqual(result['required_word_losses'], 1)

    def test_faithful_partial_output_can_use_an_equivalent_gold_alignment(self):
        result = metrics.score_case('a x a y a', 'a a', 'a x a y')
        self.assertTrue(result['preservation_valid'])
        self.assertEqual(result['correctly_deleted_tokens'], 1)
        self.assertEqual(result['gold_delete_runs'], 1)
        self.assertEqual(result['token_alignment_gold_run_count'], 2)
        complete = metrics.score_case('a x a y a', 'a a', 'a a')
        self.assertEqual(result['gold_delete_runs'], complete['gold_delete_runs'])

    def test_case_and_unicode_composition_are_lexical_formatting(self):
        result = metrics.score_case('Um, cafe\u0301 opens.', 'Café opens.', 'café opens!')
        self.assertTrue(result['complete_cleanup'])
        self.assertEqual(result['correctly_deleted_tokens'], 1)

    def test_protected_punctuation_requires_the_other_scorer(self):
        result = metrics.score_case('um open https://x.test/a-b', 'open https://x.test/a-b', 'open https://x.test/a/b')
        self.assertTrue(result['lexically_complete'])

    def test_preserve_case_and_invalid_gold(self):
        result = metrics.score_case('please keep both', 'please keep both', 'please keep both')
        self.assertIsNone(result['deletion_token_recall'])
        self.assertFalse(result['complete_cleanup'])
        self.assertTrue(result['lexically_complete'])
        with self.assertRaises(ValueError):
            metrics.score_case('keep red', 'keep blue', 'keep blue')

    def test_ambiguity_limit_is_explicit_and_not_a_fabricated_score(self):
        with patch.object(metrics, 'MAX_ALIGNMENTS', 1):
            result = metrics.score_case('I I leave', 'I leave', 'I leave')
        self.assertEqual(result['score_status'], 'unscored_alignment_limit')
        summary = metrics.summarize([result])
        self.assertEqual(summary['unscored_cases'], 1)
        self.assertEqual(summary['safe_deletion_token_recall_lower_bound'], 0)
        self.assertFalse(summary['run_metrics_cover_all_cases'])


if __name__ == '__main__':
    unittest.main()
