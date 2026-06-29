import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

// The Vue SPA is built to ./dist and embedded into the Rust binary (rust-embed).
// In dev, /api is proxied to the backend (run it with `--port 3030`).
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  server: {
    proxy: {
      // `ws: true` so the engine-analysis WebSocket (/api/engine/analyse) is
      // proxied through to the backend in dev, not just plain HTTP requests.
      '/api': { target: 'http://127.0.0.1:3030', ws: true },
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    coverage: {
      provider: 'v8',
      reporter: ['text', 'html'],
      include: ['src/**'],
    },
  },
})
