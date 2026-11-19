import { createSubscriber } from 'svelte/reactivity';

// One interval serves every reader, and runs only while something reads the clock.
const subscribe = createSubscriber((update) => {
  const interval = setInterval(update, 60_000);
  return () => clearInterval(interval);
});

/** The current time. A template or `$derived` that reads it runs again once a minute. */
export function minuteNow(): number {
  subscribe();
  return Date.now();
}
