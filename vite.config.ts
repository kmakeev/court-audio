import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

const host = process.env.TAURI_DEV_HOST;

// Конфигурация Vite для Tauri: фиксированный порт dev-сервера, без очистки
// экрана, чтобы не прятать ошибки Rust-сборки. См. docs/architecture.md.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: {
      // Не следим за Rust-ядром — пересборку ведёт cargo/tauri.
      ignored: ['**/src-tauri/**'],
    },
  },
  // Vitest: компонентные тесты UI этапа 04 (jsdom + Testing Library). Команды и
  // события Tauri замоканы в `src/test/setup.ts`.
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    exclude: ['node_modules', 'dist', 'src-tauri'],
  },
});
