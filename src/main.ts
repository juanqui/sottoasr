import './app.css';

// SottoASR is a menu bar app. This main entry point is unused in production.
// Each window (overlay, history, settings) has its own HTML/TS entry point.
// This file exists only for the Vite build system.
const app = document.getElementById('app');
if (app) {
  app.innerHTML = '<p style="padding: 2rem; color: var(--text);">SottoASR is running in the menu bar.</p>';
}
