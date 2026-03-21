import OverlayPill from './lib/components/overlay-pill.svelte';
import { mount } from 'svelte';

const app = mount(OverlayPill, { target: document.getElementById('app')! });
export default app;
