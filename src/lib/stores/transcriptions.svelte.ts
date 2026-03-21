import {
  getTranscriptions as fetchTranscriptions,
  deleteTranscription as removeTranscription,
  clearTranscriptions as removeAllTranscriptions,
} from '../utils/tauri';

import type { Transcription } from '../utils/tauri';

class TranscriptionStore {
  /** All transcriptions in reverse-chronological order */
  items: Transcription[] = $state([]);

  /** Whether the store has been loaded from the backend */
  loaded: boolean = $state(false);

  /** The most recent transcription, or null if none exist */
  get last(): Transcription | null {
    return this.items.length > 0 ? this.items[0] : null;
  }

  /** Load transcriptions from the Tauri backend */
  async load() {
    try {
      this.items = await fetchTranscriptions();
      this.loaded = true;
    } catch (err) {
      console.error('Failed to load transcriptions:', err);
    }
  }

  /** Add a transcription to the top of the list (most recent first) */
  add(transcription: Transcription) {
    this.items = [transcription, ...this.items];
  }

  /** Delete a transcription by id */
  async delete(id: string) {
    try {
      await removeTranscription(id);
      this.items = this.items.filter((t) => t.id !== id);
    } catch (err) {
      console.error('Failed to delete transcription:', err);
    }
  }

  /** Clear all transcriptions */
  async clear() {
    try {
      await removeAllTranscriptions();
      this.items = [];
    } catch (err) {
      console.error('Failed to clear transcriptions:', err);
    }
  }
}

export const transcriptionStore = new TranscriptionStore();
