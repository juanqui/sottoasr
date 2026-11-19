import OnboardingView from './lib/components/onboarding-view.svelte';
import { mount } from 'svelte';
import { forwardConsoleToLog } from './lib/utils/console-log';

forwardConsoleToLog();
const app = mount(OnboardingView, { target: document.getElementById('app')! });
export default app;
