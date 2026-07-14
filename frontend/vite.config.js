import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// Build output is embedded into the photon-server binary (rust-embed) from `dist/`.
// During dev, /api is proxied to the running photon-server.
// `defineConfig` is imported from `vitest/config` (it extends Vite's config) so the
// same file carries both the Vite build config and the Vitest test config.
export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8080',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'jsdom',
    globals: true,
  },
})
