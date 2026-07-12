import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import OperatorApp from './OperatorApp'
// Initialize i18next before the app renders (issue #764), matching main.tsx so
// `useTranslation()` works synchronously on first paint.
import './lib/i18n'
import '@botho/ui/styles/theme.css'

// NOTE (#772, §8.3.1 option (a)): this entry deliberately does NOT register the
// PWA service worker.
//
// The main SPA registers a `registerType: 'autoUpdate'` service worker in
// `main.tsx`, which auto-fetches and activates a new bundle after a deploy. SRI
// only pins *sub-resources* referenced by a document — it cannot protect the
// top-level HTML document itself. If the operator document were under SW
// control, an auto-updated worker could silently swap `operator.html` (and thus
// the integrity hashes it pins), defeating the whole point of the split.
//
// By not registering the SW here, and by excluding `operator.html` + its chunks
// from the Workbox precache glob in `vite.config.ts`, the browser always fetches
// `/operator` fresh over the network. The `integrity=` attributes injected onto
// this document's <script>/<link> tags then guarantee the referenced chunks are
// byte-for-byte the published ones or the browser refuses to execute them.

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <OperatorApp />
  </StrictMode>,
)
