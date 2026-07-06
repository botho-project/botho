import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { registerSW } from 'virtual:pwa-register'
import App from './App'
import { shouldReloadOnControllerChange } from './lib/sw-reload'
import '@botho/ui/styles/theme.css'

// Auto-update the PWA after a deploy.
//
// vite-plugin-pwa is configured with `registerType: 'autoUpdate'`, so this
// generated `registerSW` automatically applies any new service worker it finds
// (the SW also uses skipWaiting + clientsClaim). When the new worker takes
// control the browser fires `controllerchange`; we reload exactly once so the
// visitor gets the freshly deployed app within one navigation — no manual hard
// refresh. The module-level `reloading` guard prevents a reload loop.
//
// EXCEPTION (#654): the `/pay` and `/claim` pages read a one-time fragment from
// the URL and strip it on mount for privacy (#589). On a first visit the SW
// activates a few seconds later — AFTER the strip — so reloading there would
// land on a fragment-less URL and render a valid link as "not found". Skip the
// reload on those routes; the visitor gets the new build on their next
// navigation. See `shouldReloadOnControllerChange`.
if ('serviceWorker' in navigator) {
  let reloading = false
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (reloading) return
    if (!shouldReloadOnControllerChange(window.location.pathname)) return
    reloading = true
    window.location.reload()
  })
  registerSW({ immediate: true })
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
