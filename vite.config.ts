import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// @ts-expect-error process is a node global available at config time
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
});
