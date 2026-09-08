import {
  getTranscriptions as fetchTranscriptions,
  deleteTranscription as removeTranscription,
  clearTranscriptions as removeAllTranscriptions,
} from '../utils/tauri';
import type { Transcription, TranscriptionEvent } from '../utils/tauri';

export class TranscriptionStore {
  items: Transcription[] = $state([]);
  loaded = $state(false);
  loading = $state(false);
  error = $state('');
  private generation = 0;
  private removedIds = new Set<string>();
  private pendingArrivals: Map<string, Transcription> | null = null;

  get last(): Transcription | null { return this.items[0] ?? null; }

  async load() {
    const generation = ++this.generation;
    const arrivals = new Map<string, Transcription>();
    this.pendingArrivals = arrivals;
    this.loading = true;
    this.error = '';
    try {
      const fetched = await fetchTranscriptions();
      if (generation !== this.generation) return;
      // The snapshot is authoritative; preserve only events received during
      // this read, not stale rows from a previous snapshot or eviction.
      const items = new Map(fetched.filter((item) => !this.removedIds.has(item.id)).map((item) => [item.id, item]));
      for (const item of arrivals.values()) if (!this.removedIds.has(item.id)) items.set(item.id, item);
      this.items = [...items.values()].sort((a, b) => b.created_at.localeCompare(a.created_at));
      this.loaded = true;
    } catch (error) {
      if (generation === this.generation) this.error = `Could not load history: ${String(error)}`;
    } finally {
      if (generation === this.generation) { this.loading = false; this.pendingArrivals = null; }
    }
  }

  add(event: TranscriptionEvent) {
    const { removed_ids = [], ...transcription } = event;
    removed_ids.forEach((id) => { this.removedIds.add(id); this.pendingArrivals?.delete(id); });
    if (this.removedIds.has(transcription.id)) {
      // The record itself may have been deleted after storage but before its
      // event arrived. Its acknowledged evictions still happened durably.
      if (removed_ids.length) this.items = this.items.filter((item) => !this.removedIds.has(item.id));
      return;
    }
    this.pendingArrivals?.set(transcription.id, transcription);
    this.items = [transcription, ...this.items.filter((item) => item.id !== transcription.id && !this.removedIds.has(item.id))];
  }

  async delete(id: string) {
    await removeTranscription(id);
    this.removedIds.add(id);
    this.items = this.items.filter((item) => item.id !== id);
  }

  async clear() {
    // Only remove IDs the backend actually deleted. New entries may arrive
    // during this transaction or before its response reaches the window.
    const removed = await removeAllTranscriptions();
    removed.forEach((id) => this.removedIds.add(id));
    this.items = this.items.filter((item) => !this.removedIds.has(item.id));
  }

  invalidateLoad() { ++this.generation; this.pendingArrivals = null; }
}

export const transcriptionStore = new TranscriptionStore();
