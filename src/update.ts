import UpdateView from './lib/components/update-view.svelte';
import { mount } from 'svelte';
import { forwardConsoleToLog } from './lib/utils/console-log';

forwardConsoleToLog();
const app = mount(UpdateView, { target: document.getElementById('app')! });
export default app;
