<script lang="ts">
  import { createDefaultSettings, settingsStore } from '../stores/settings.svelte';
  import ShortcutRecorder from './shortcut-recorder.svelte';
  import SettingsToggle from './settings-toggle.svelte';
  import type { Settings } from '../utils/tauri';

  type RequiredShortcut = 'push_to_talk_shortcut' | 'toggle_shortcut' | 'cancel_shortcut' | 'open_settings_shortcut';
  type AlternateShortcut = 'push_to_talk_shortcut_alt' | 'toggle_shortcut_alt' | 'cancel_shortcut_alt';

  const defaults: Settings = createDefaultSettings();
  let active = $state<string | null>(null);
  const shortcuts: { key: RequiredShortcut; alternate?: AlternateShortcut; label: string; hint: string }[] = [
    { key: 'push_to_talk_shortcut', alternate: 'push_to_talk_shortcut_alt', label: 'Push to talk', hint: 'Hold to record, release to transcribe.' },
    { key: 'toggle_shortcut', alternate: 'toggle_shortcut_alt', label: 'Toggle recording', hint: 'Press to start, press again to stop.' },
    { key: 'cancel_shortcut', alternate: 'cancel_shortcut_alt', label: 'Cancel recording', hint: 'Stop without pasting.' },
    { key: 'open_settings_shortcut', label: 'Open Settings', hint: 'Works even when the menu bar icon is hidden.' },
  ];
</script>

<section class="setting-card">
  <h3>Keyboard shortcuts</h3>
  {#each shortcuts as shortcut}
    <div class="shortcut-field" role="group" aria-label={shortcut.label}>
      <span class="field-label">{shortcut.label}</span>
      <div class="shortcut-pair">
        <ShortcutRecorder
          label={`${shortcut.label} shortcut`}
          value={settingsStore.current[shortcut.key]}
          defaultValue={defaults[shortcut.key]}
          onchange={(value) => settingsStore.update(shortcut.key, value)}
          disabled={active !== null && active !== shortcut.key}
          onrecordstart={() => { active = shortcut.key; }} onrecordend={() => { active = null; }}
        />
        {#if shortcut.alternate}
          {@const alternate = shortcut.alternate}
          <ShortcutRecorder
            label={`Alternate ${shortcut.label.toLowerCase()} shortcut`} placeholder="Add alternate"
            value={settingsStore.current[alternate] ?? ''}
            onchange={(value) => settingsStore.update(alternate, value || null)}
            disabled={active !== null && active !== alternate}
            onrecordstart={() => { active = alternate; }} onrecordend={() => { active = null; }}
          />
        {/if}
      </div>
      <p class="setting-hint">{shortcut.hint}</p>
    </div>
  {/each}
</section>
<section class="setting-card">
  <h3>Startup and updates</h3>
  <SettingsToggle label="Launch at login" hint="Start quietly in the menu bar."
    checked={settingsStore.current.launch_at_login} onchange={(value) => settingsStore.update('launch_at_login', value)} />
  <SettingsToggle label="Check for updates" hint="Check periodically for new versions of SottoASR."
    checked={settingsStore.current.auto_check_updates} onchange={(value) => settingsStore.update('auto_check_updates', value)} />
</section>
