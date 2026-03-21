import AboutView from './lib/components/about-view.svelte';
import { mount } from 'svelte';

const app = mount(AboutView, { target: document.getElementById('app')! });
export default app;
