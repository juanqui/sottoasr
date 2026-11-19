import AboutView from './lib/components/about-view.svelte';
import { mount } from 'svelte';
import { forwardConsoleToLog } from './lib/utils/console-log';

forwardConsoleToLog();
const app = mount(AboutView, { target: document.getElementById('app')! });
export default app;
