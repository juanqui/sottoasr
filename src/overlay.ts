import OverlayPill from './lib/components/overlay-pill.svelte';
import { mount } from 'svelte';
import { forwardConsoleToLog } from './lib/utils/console-log';

forwardConsoleToLog();
const app = mount(OverlayPill, { target: document.getElementById('app')! });
export default app;
