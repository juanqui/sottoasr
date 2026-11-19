import SettingsPanel from './lib/components/settings-panel.svelte';
import { mount } from 'svelte';
import { forwardConsoleToLog } from './lib/utils/console-log';

forwardConsoleToLog();
const app = mount(SettingsPanel, { target: document.getElementById('app')! });
export default app;
