import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:9870',
      '/health': 'http://127.0.0.1:9870'
    }
  },
  test: {
    include: ['src/**/*.test.ts']
  }
});
