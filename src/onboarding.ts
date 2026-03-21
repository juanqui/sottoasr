import OnboardingView from './lib/components/onboarding-view.svelte';
import { mount } from 'svelte';

const app = mount(OnboardingView, { target: document.getElementById('app')! });
export default app;
