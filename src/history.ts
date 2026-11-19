import HistoryView from './lib/components/history-view.svelte';
import { mount } from 'svelte';
import { forwardConsoleToLog } from './lib/utils/console-log';

forwardConsoleToLog();
const app = mount(HistoryView, { target: document.getElementById('app')! });
export default app;
