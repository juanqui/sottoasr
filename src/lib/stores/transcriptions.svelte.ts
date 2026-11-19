import {
  getTranscriptions as fetchTranscriptions,
  deleteTranscription as removeTranscription,
  clearTranscriptions as removeAllTranscriptions,
} from '../utils/tauri';
import type { Transcription, TranscriptionEvent } from '../utils/tauri';

export class TranscriptionStore {
  // Up to 5,000 entries; every change replaces the array, so deep reactivity is not needed.
  items: Transcription[] = $state.raw([]);
  loaded = $state(false);
  loading = $state(false);
  error = $state('');
  private generation = 0;
  // IDs the backend deleted, with the time this window learned of it. A late
  // event for a deleted entry must not bring it back, but a save made after
  // the deletion (a recovered recording) must show.
  private removedIds = new Map<string, number>();
  private pendingArrivals: Map<string, Transcription> | null = null;

  get last(): Transcription | null { return this.items[0] ?? null; }

  /** True when this copy of an entry predates the backend deleting it. */
  private isRemoved(item: Transcription) {
    const removedAt = this.removedIds.get(item.id);
    if (removedAt === undefined) return false;
    return !(item.saved_at && Date.parse(item.saved_at) > removedAt);
  }

  private markRemoved(ids: string[]) {
    const now = Date.now();
    ids.forEach((id) => { this.removedIds.set(id, now); this.pendingArrivals?.delete(id); });
  }

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
      const items = new Map(fetched.filter((item) => !this.isRemoved(item)).map((item) => [item.id, item]));
      for (const item of arrivals.values()) if (!this.isRemoved(item)) items.set(item.id, item);
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
    this.markRemoved(removed_ids);
    if (this.isRemoved(transcription)) {
      // The record itself may have been deleted after storage but before its
      // event arrived. Its acknowledged evictions still happened durably.
      if (removed_ids.length) this.items = this.items.filter((item) => !this.removedIds.has(item.id));
      return;
    }
    this.removedIds.delete(transcription.id);
    this.pendingArrivals?.set(transcription.id, transcription);
    const items = this.items.filter((item) => item.id !== transcription.id && !this.removedIds.has(item.id));
    const position = items.findIndex((item) => item.created_at.localeCompare(transcription.created_at) <= 0);
    items.splice(position === -1 ? items.length : position, 0, transcription);
    this.items = items;
  }

  /**
   * Show a transcription the backend has just saved, such as a recovered
   * recording, even when an earlier entry with its ID was deleted.
   */
  addSaved(transcription: Transcription) {
    this.removedIds.delete(transcription.id);
    this.add(transcription);
  }

  async delete(id: string) {
    await removeTranscription(id);
    this.markRemoved([id]);
    this.items = this.items.filter((item) => item.id !== id);
  }

  async clear() {
    // Only remove IDs the backend actually deleted. New entries may arrive
    // during this transaction or before its response reaches the window.
    const removed = await removeAllTranscriptions();
    this.markRemoved(removed);
    this.items = this.items.filter((item) => !this.removedIds.has(item.id));
  }

  /** Drop entries the backend already deleted, such as those removed by retention. */
  remove(ids: string[]) {
    if (!ids.length) return;
    this.markRemoved(ids);
    this.items = this.items.filter((item) => !this.removedIds.has(item.id));
  }

  invalidateLoad() { ++this.generation; this.pendingArrivals = null; }
}

export const transcriptionStore = new TranscriptionStore();
