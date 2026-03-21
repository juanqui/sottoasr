import HistoryView from './lib/components/history-view.svelte';
import { mount } from 'svelte';

const app = mount(HistoryView, { target: document.getElementById('app')! });
export default app;
