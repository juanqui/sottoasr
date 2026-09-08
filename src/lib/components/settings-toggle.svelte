<script lang="ts">
  interface Props { label: string; hint?: string; checked: boolean; disabled?: boolean; onchange: (checked: boolean) => void; }
  let { label, hint, checked, disabled = false, onchange }: Props = $props();
</script>
<label class="setting-toggle">
  <span class="setting-description"><strong>{label}</strong>{#if hint}<span>{hint}</span>{/if}</span>
  <input type="checkbox" {checked} {disabled} onchange={(event) => onchange(event.currentTarget.checked)} />
  <span class="toggle-track" aria-hidden="true"></span>
</label>

<style>
  .setting-toggle { display: flex; gap: 18px; align-items: center; position: relative; padding: 14px 0; cursor: pointer; }
  .setting-description { flex: 1; min-width: 0; display: grid; gap: 4px; }
  strong { color: var(--text-bright); font-size: 13px; font-weight: 500; }
  .setting-description > span { color: var(--text-dim); font-size: 12px; line-height: 1.5; }
  input { position: absolute; right: 0; width: 38px; height: 24px; margin: 0; opacity: 0; }
  .toggle-track { flex: 0 0 38px; height: 24px; border-radius: 20px; background: var(--border-hover); pointer-events: none; transition: background 120ms; }
  .toggle-track::after { content: ''; display: block; width: 18px; height: 18px; margin: 3px; border-radius: 50%; background: white; transition: transform 120ms; }
  input:checked + .toggle-track { background: var(--accent); }
  input:checked + .toggle-track::after { transform: translateX(14px); }
  input:focus-visible + .toggle-track { outline: 2px solid var(--accent); outline-offset: 3px; }
  input:disabled + .toggle-track { opacity: .55; }
</style>
