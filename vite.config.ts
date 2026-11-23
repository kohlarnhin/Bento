import { createRequire } from 'node:module';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const require = createRequire(import.meta.url);

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [{
      // Re-minifying xterm 6.0.0's ESM drops a declaration in its mode-query handler.
      find: /^@xterm\/xterm$/,
      replacement: require.resolve('@xterm/xterm'),
    }],
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
});
