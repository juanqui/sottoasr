import SettingsPanel from './lib/components/settings-panel.svelte';
import { mount } from 'svelte';

const app = mount(SettingsPanel, { target: document.getElementById('app')! });
export default app;
