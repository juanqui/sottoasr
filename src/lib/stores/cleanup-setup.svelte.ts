import { getLlmStatus, prepareLlmModel, repairLlmRuntime } from '../utils/tauri';
import type { LlmStatus } from '../utils/tauri';

const READY_NOTICE = 'Ready. Save to enable AI cleanup.';
const REPAIRED_NOTICE = 'Cleanup runtime repaired.';

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
    if (this.loading || this.disposed) return;
    const request = ++this.statusRequest;
    this.loading = true;
    try {
      const status = await getLlmStatus();
      if (!this.disposed && request === this.statusRequest) { this.status = status; if (!this.pending) this.error = status.setup_error ?? ''; }
    } catch (error) {
      if (!this.disposed && request === this.statusRequest) { this.status = null; this.error = String(error); }
    } finally {
      if (!this.disposed && request === this.statusRequest) this.loading = false;
    }
  }

  async enable(onReady: () => void) {
    await this.run(
      prepareLlmModel,
      (status) => {
        if (!status.loaded || !status.downloaded || status.setup_error) {
          throw new Error(status.setup_error || 'The cleanup model is not ready. Try setup again.');
        }
        onReady();
      },
      READY_NOTICE,
    );
  }

  /** Rebuild the runtime after a broken-verdict failure. */
  async repair() {
    await this.run(
      repairLlmRuntime,
      (status) => {
        if (!status.loaded || status.setup_error) {
          throw new Error(status.setup_error || 'The cleanup runtime is still not ready. Try again.');
        }
      },
      REPAIRED_NOTICE,
    );
  }

  /**
   * Shared envelope for the two explicit preparation actions: one owner at a
   * time, a stale intent never overwrites a newer one, and the acknowledgement
   * always outranks a status poll that began during the work.
   */
  private async run(
    action: () => Promise<LlmStatus>,
    accept: (status: LlmStatus) => void,
    notice: string,
  ) {
    if (this.pending) return;
    const intent = ++this.intent;
    ++this.statusRequest;
    this.loading = false;
    this.pending = true;
    this.error = '';
    this.notice = '';
    try {
      const status = await action();
      if (this.disposed || intent !== this.intent) return;
      // A poll begun during preparation may still describe the old unloaded
      // process. The preparation acknowledgement is newer and authoritative.
      ++this.statusRequest;
      this.loading = false;
      this.status = status;
      accept(status);
      this.notice = notice;
    } catch (error) {
      if (!this.disposed && intent === this.intent) this.error = error instanceof Error ? error.message : String(error);
    } finally {
      if (!this.disposed && intent === this.intent) this.pending = false;
    }
  }

  async prepare() {
    await this.enable(() => {});
    this.acknowledgeSaved(this.status?.enabled === true);
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
