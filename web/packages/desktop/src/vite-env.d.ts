/// <reference types="vite/client" />
//
// TypeScript 7 reports `error TS2882: Cannot find module or type declarations
// for side-effect import` for bare `import './x.css'` statements that no
// ambient declaration covers (TS 5.x silently accepted them). Vite's client
// types declare `*.css` (and the other asset extensions this app imports), so
// pulling them in here restores resolution for `main.tsx`'s
// `@botho/ui/styles/theme.css` and `network-graph.tsx`'s
// `@xyflow/react/dist/style.css`. Mirrors packages/web-wallet/src/vite-env.d.ts.
