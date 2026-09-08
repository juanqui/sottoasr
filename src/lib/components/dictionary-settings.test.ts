import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import DictionarySettings from './dictionary-settings.svelte';

let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  target?.remove();
});

describe('dictionary editor', () => {
  it('adds an empty draft without changing the supplied entries', () => {
    const entries: [] = [];
    const onchange = vi.fn();
    target = document.createElement('div');
    document.body.append(target);
    component = mount(DictionarySettings, { target, props: { entries, onchange } });
    flushSync();

    target.querySelector<HTMLButtonElement>('.add')!.click();

    expect(onchange).toHaveBeenCalledWith([{ heard: '', replacement: '' }]);
    expect(entries).toEqual([]);
  });

  it('edits and removes rows immutably', () => {
    const entries = [{ heard: 'Quen', replacement: 'Qwen' }];
    const onchange = vi.fn();
    target = document.createElement('div');
    document.body.append(target);
    component = mount(DictionarySettings, { target, props: { entries, onchange } });
    flushSync();

    const input = target.querySelector<HTMLInputElement>('input[aria-label="Heard alias 1"]')!;
    input.value = 'Quen next';
    input.dispatchEvent(new Event('input', { bubbles: true }));

    expect(onchange).toHaveBeenLastCalledWith([{ heard: 'Quen next', replacement: 'Qwen' }]);
    expect(entries).toEqual([{ heard: 'Quen', replacement: 'Qwen' }]);
    target.querySelector<HTMLButtonElement>('.remove')!.click();
    expect(onchange).toHaveBeenLastCalledWith([]);
    expect(entries).toHaveLength(1);
  });
});
