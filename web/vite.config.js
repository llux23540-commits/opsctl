import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [vue()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    proxy: {
      // dev: forward /api to the Rust server
      '/api': { target: 'http://127.0.0.1:8443', changeOrigin: true, ws: true },
    },
  },
  build: { outDir: 'dist' },
});
