import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { registerSW } from 'virtual:pwa-register'
import App from './App'
import '@botho/ui/styles/theme.css'

// Auto-update the PWA after a deploy.
//
// vite-plugin-pwa is configured with `registerType: 'autoUpdate'`, so this
// generated `registerSW` automatically applies any new service worker it finds
// (the SW also uses skipWaiting + clientsClaim). When the new worker takes
// control the browser fires `controllerchange`; we reload exactly once so the
// visitor gets the freshly deployed app within one navigation — no manual hard
// refresh. The module-level `reloading` guard prevents a reload loop.
if ('serviceWorker' in navigator) {
  let reloading = false
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (reloading) return
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
