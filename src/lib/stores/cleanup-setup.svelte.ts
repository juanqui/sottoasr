import { getLlmStatus, prepareLlmModel } from '../utils/tauri';
import type { LlmStatus } from '../utils/tauri';

const READY_NOTICE = 'Ready. Save to enable AI cleanup.';

/** Setup belongs to the settings window, so changing sections preserves intent. */
export class CleanupSetup {
  status: LlmStatus | null = $state(null);
  loading = $state(false);
  pending = $state(false);
  error = $state('');
  notice = $state('');
  private disposed = false;
  private intent = 0;
  private statusRequest = 0;

  async refresh() {
    const request = ++this.statusRequest;
    this.loading = true;
    try {
      const status = await getLlmStatus();
      if (!this.disposed && request === this.statusRequest) { this.status = status; if (!this.pending) this.error = status.setup_error ?? ''; }
    } catch (error) {
      if (!this.disposed && request === this.statusRequest) this.error = String(error);
    } finally {
      if (!this.disposed && request === this.statusRequest) this.loading = false;
    }
  }

  async enable(onReady: () => void) {
    if (this.pending) return;
    const intent = ++this.intent;
    ++this.statusRequest;
    this.loading = false;
    this.pending = true;
    this.error = '';
    this.notice = '';
    try {
      const status = await prepareLlmModel();
      if (this.disposed || intent !== this.intent) return;
      this.status = status;
      if (!status.loaded || !status.downloaded || status.setup_error) {
        throw new Error(status.setup_error || 'The cleanup model is not ready. Try setup again.');
      }
      onReady();
      this.notice = READY_NOTICE;
    } catch (error) {
      if (!this.disposed && intent === this.intent) this.error = error instanceof Error ? error.message : String(error);
    } finally {
      if (!this.disposed && intent === this.intent) this.pending = false;
    }
  }

  cancel() {
    ++this.intent;
    if (this.pending) this.notice = 'Activation cancelled. Setup may finish in the background; cleanup stays off.';
    else if (this.notice === READY_NOTICE) this.notice = '';
    this.pending = false;
    this.error = '';
  }

  acknowledgeSaved(enabled: boolean) {
    if (enabled && !this.pending && this.notice === READY_NOTICE) this.notice = '';
  }

  dispose() { this.cancel(); this.disposed = true; }
}
