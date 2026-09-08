import { getVocabularyStatus, prepareVocabularyModel } from '../utils/tauri';
import type { VocabularyStatus } from '../utils/tauri';

export class VocabularySetup {
  status: VocabularyStatus | null = $state(null);
  loading = $state(false);
  pending = $state(false);
  error = $state('');
  private generation = 0;
  private disposed = false;

  async refresh() {
    const generation = ++this.generation;
    this.loading = true;
    try {
      const status = await getVocabularyStatus();
      if (!this.disposed && generation === this.generation) {
        this.status = status;
        this.error = status.error ?? '';
      }
    } catch (error) {
      if (!this.disposed && generation === this.generation) this.error = String(error);
    } finally {
      if (!this.disposed && generation === this.generation) this.loading = false;
    }
  }

  async retry() {
    if (this.pending) return;
    this.pending = true;
    this.error = '';
    try {
      await prepareVocabularyModel();
      if (!this.disposed) await this.refresh();
    } catch (error) {
      if (!this.disposed) this.error = String(error);
    } finally {
      if (!this.disposed) this.pending = false;
    }
  }

  dispose() { this.disposed = true; ++this.generation; }
}
