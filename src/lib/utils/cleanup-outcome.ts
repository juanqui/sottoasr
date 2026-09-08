import type { LlmCleanupStatus } from './tauri';

export function cleanupOutcome(status?: LlmCleanupStatus) {
  const reason = status && 'detail' in status && 'reason' in status.detail ? status.detail.reason : '';
  // Old releases stored validation rejections under the generic failed status.
  const rejected = status?.kind === 'rejected' || (status?.kind === 'failed' &&
    /^(Proposal requires |Protected span changed|Ambiguous source reconstruction|Word limit or word addition)/.test(reason));
  if (rejected) return { label: 'Original kept', issue: true, detail: `The model responded, but its edits did not pass preservation checks. ${reason}` };
  switch (status?.kind) {
    case 'applied': return { label: 'AI cleaned', issue: false, detail: `Validated cleanup applied in ${(status.detail.elapsed_ms / 1000).toFixed(1)}s. Original speech recognition text is retained.` };
    case 'failed': return { label: 'Cleanup failed', issue: true, detail: reason };
    case 'unavailable': return { label: 'Cleanup unavailable', issue: true, detail: reason };
    case 'timed_out': return { label: 'Cleanup timed out', issue: true, detail: `The original was kept after ${(status.detail.elapsed_ms / 1000).toFixed(1)}s. The model will restart on the next recording.` };
    case 'no_changes': return { label: 'Unchanged', issue: false, detail: 'Cleanup made no changes.' };
    case 'disabled': return { label: 'Cleanup off', issue: false, detail: 'AI cleanup was disabled for this recording.' };
    case 'skipped_no_candidates': return { label: 'Original kept', issue: false, detail: 'AI suggestions did not run: no edits qualified, or the transcript exceeded cleanup limits.' };
    case 'skipped_too_short': return { label: 'Original kept', issue: false, detail: 'This older version skipped cleanup for short recordings.' };
    case 'suggested': return { label: 'Suggestion to review', issue: false, detail: 'An older experimental suggestion is available for review. It was not applied.' };
    default: return { label: 'Transcript saved', issue: false, detail: 'This entry has no recorded cleanup diagnostics.' };
  }
}
