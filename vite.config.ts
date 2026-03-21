import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { resolve } from 'path';

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 14517,
    strictPort: true,
  },
  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        overlay: resolve(__dirname, 'overlay.html'),
        history: resolve(__dirname, 'history.html'),
        settings: resolve(__dirname, 'settings.html'),
        onboarding: resolve(__dirname, 'onboarding.html'),
        about: resolve(__dirname, 'about.html'),
      },
    },
  },
});
