import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  // Tauri expects a fixed port, fallback to next available if taken
  server: {
    port: 1420,
    strictPort: true,
  },
  // Build configuration for Tauri
  build: {
    // Tauri supports es2021
    target: process.env.TAURI_PLATFORM == 'windows' ? 'chrome105' : 'safari13',
    // Use Vite 8's native minifier; the legacy esbuild pass cannot lower
    // destructuring for our unchanged Safari 13 target. Keep debug readable.
    minify: !process.env.TAURI_DEBUG ? 'oxc' : false,
    // Produce sourcemaps for debug builds
    sourcemap: !!process.env.TAURI_DEBUG,
  },
  // Prevent clearing the terminal (Tauri uses it for logs)
  clearScreen: false,
})
