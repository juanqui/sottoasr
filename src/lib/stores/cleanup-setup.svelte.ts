import { getLlmStatus, prepareLlmModel, repairLlmRuntime } from '../utils/tauri';
import type { DownloadProgress, LlmStatus } from '../utils/tauri';

const READY_NOTICE = 'Ready. Save to enable AI cleanup.';
const REPAIRED_NOTICE = 'Cleanup runtime repaired.';

/** Setup belongs to the settings window, so changing sections preserves intent. */
export class CleanupSetup {
  status: LlmStatus | null = $state(null);
  loading = $state(false);
  pending = $state(false);
  error = $state('');
  notice = $state('');
  /** Latest `llm-download-progress` payload of the preparation in flight. */
  progress: DownloadProgress | null = $state(null);
  /** Set while the preparation in flight is for a change of the draft engine to "On this Mac". */
  selectingLocal = $state(false);
  /** Why the last change of the draft engine to "On this Mac" failed. The draft kept AWS Bedrock. */
  selectError = $state('');
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
    await this.prepareLocal(onReady, false);
  }

  /**
   * Prepare the local model for a change of the draft engine to "On this Mac".
   * `onReady` changes the draft. It runs only after the preparation succeeds.
   */
  async selectLocal(onReady: () => void) {
    await this.prepareLocal(onReady, true);
  }

  private async prepareLocal(onReady: () => void, selecting: boolean) {
    await this.run(
      prepareLlmModel,
      (status) => {
        if (!status.loaded || !status.downloaded || status.setup_error) {
          throw new Error(status.setup_error || 'The cleanup model is not ready. Try setup again.');
        }
        onReady();
      },
      READY_NOTICE,
      selecting,
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
    selecting = false,
  ) {
    if (this.pending) return;
    const intent = ++this.intent;
    ++this.statusRequest;
    this.loading = false;
    this.pending = true;
    this.selectingLocal = selecting;
    this.error = '';
    this.selectError = '';
    this.notice = '';
    this.progress = null;
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
      if (!this.disposed && intent === this.intent) {
        const text = error instanceof Error ? error.message : String(error);
        if (selecting) this.selectError = text;
        else this.error = text;
      }
    } finally {
      if (!this.disposed && intent === this.intent) { this.pending = false; this.selectingLocal = false; this.progress = null; }
    }
  }

  async prepare() {
    await this.enable(() => {});
    this.acknowledgeSaved(this.status?.enabled === true);
  }

  cancel() {
    ++this.intent;
    // A cancelled change of the engine leaves cleanup as it was, so it needs no notice.
    if (this.pending && !this.selectingLocal) this.notice = 'Activation cancelled. Setup may finish in the background; cleanup stays off.';
    else if (this.notice === READY_NOTICE) this.notice = '';
    this.pending = false;
    this.selectingLocal = false;
    this.progress = null;
    this.error = '';
    this.selectError = '';
  }

  /** Progress is shown only for the preparation this window started. */
  reportProgress(progress: DownloadProgress) {
    if (this.pending && !this.disposed) this.progress = progress;
  }

  acknowledgeSaved(enabled: boolean) {
    if (enabled && !this.pending && this.notice === READY_NOTICE) this.notice = '';
  }

  dispose() { this.cancel(); this.disposed = true; }
}
