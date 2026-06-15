import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { VitePWA } from 'vite-plugin-pwa'
import path from 'path'

// Target for the same-origin `/rpc` proxy. Defaults to the live seed node for
// local dev / preview, but the e2e suite overrides it (E2E_RPC_PROXY_TARGET) to
// point at a local JSON-RPC mock so the explorer specs are hermetic and do not
// depend on the shared public node being responsive. See web/e2e/serve-rpc-mock.mjs.
const RPC_PROXY_TARGET = process.env.E2E_RPC_PROXY_TARGET || 'https://seed.botho.io'

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: 'autoUpdate',
      includeAssets: ['favicon.ico', 'apple-touch-icon.png', 'mask-icon.svg'],
      manifest: {
        name: 'Botho Wallet',
        short_name: 'Botho',
        description: 'Privacy-first cryptocurrency wallet',
        theme_color: '#06b6d4',
        background_color: '#0a0b0f',
        display: 'standalone',
        orientation: 'portrait',
        scope: '/',
        start_url: '/',
        icons: [
          {
            src: 'pwa-192x192.png',
            sizes: '192x192',
            type: 'image/png',
          },
          {
            src: 'pwa-512x512.png',
            sizes: '512x512',
            type: 'image/png',
          },
          {
            src: 'pwa-512x512.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'any maskable',
          },
        ],
      },
      workbox: {
        globPatterns: ['**/*.{js,css,html,ico,png,svg,woff2}'],
        navigateFallbackDenylist: [/\.pdf$/],
        runtimeCaching: [
          {
            urlPattern: /^https:\/\/fonts\.googleapis\.com\/.*/i,
            handler: 'CacheFirst',
            options: {
              cacheName: 'google-fonts-cache',
              expiration: {
                maxEntries: 10,
                maxAgeSeconds: 60 * 60 * 24 * 365, // 1 year
              },
              cacheableResponse: {
                statuses: [0, 200],
              },
            },
          },
        ],
      },
    }),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 3000,
    proxy: {
      '/api': {
        target: 'https://seed.botho.io',
        changeOrigin: true,
      },
      // Same-origin proxy to the seed node read RPC. Used during local dev and
      // e2e (build with VITE_RPC_ENDPOINT=/rpc) so the app reaches a real node
      // without depending on cross-origin CORS.
      '/rpc': {
        target: RPC_PROXY_TARGET,
        changeOrigin: true,
      },
    },
  },
  // `vite preview` (used by the e2e webServer) needs the same /rpc proxy as the
  // dev server so the served production build can reach the seed node RPC.
  preview: {
    proxy: {
      '/rpc': {
        target: RPC_PROXY_TARGET,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
  },
})
