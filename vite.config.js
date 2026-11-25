import { defineConfig } from 'vite';

export default defineConfig({
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 22869,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
});
