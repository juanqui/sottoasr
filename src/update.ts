import UpdateView from './lib/components/update-view.svelte';
import { mount } from 'svelte';

const app = mount(UpdateView, { target: document.getElementById('app')! });
export default app;
