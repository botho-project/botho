/**
 * Minimal static file server for the faucet site (infra/faucet/web).
 *
 * Used by the Playwright `webServer` config to serve the faucet site locally
 * during e2e runs, so the faucet specs do not depend on the live
 * faucet.botho.io deployment being up.
 *
 * No external dependencies — uses only Node's built-in http/fs modules.
 */
import { createServer } from 'node:http'
import { readFile, stat } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(__dirname, '../../infra/faucet/web')
const PORT = Number(process.env.FAUCET_PORT ?? 4174)

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? '/', `http://localhost:${PORT}`)
    let pathname = decodeURIComponent(url.pathname)

    // The faucet site issues same-origin POSTs to /rpc. There is no faucet
    // backend in local e2e, so respond with a JSON-RPC error the client
    // handles gracefully (renders #status-error / #result-banner).
    if (pathname === '/rpc') {
      res.writeHead(503, { 'Content-Type': 'application/json' })
      res.end(
        JSON.stringify({
          jsonrpc: '2.0',
          error: { code: -32000, message: 'Faucet backend not available in local e2e' },
          id: null,
        })
      )
      return
    }

    if (pathname === '/' || pathname.endsWith('/')) {
      pathname += 'index.html'
    }

    const filePath = path.join(ROOT, path.normalize(pathname))
    if (!filePath.startsWith(ROOT)) {
      res.writeHead(403)
      res.end('Forbidden')
      return
    }

    const info = await stat(filePath).catch(() => null)
    if (!info || !info.isFile()) {
      // SPA-style fallback to index.html
      const fallback = await readFile(path.join(ROOT, 'index.html')).catch(() => null)
      if (fallback) {
        res.writeHead(200, { 'Content-Type': MIME['.html'] })
        res.end(fallback)
        return
      }
      res.writeHead(404)
      res.end('Not found')
      return
    }

    const ext = path.extname(filePath)
    const body = await readFile(filePath)
    res.writeHead(200, { 'Content-Type': MIME[ext] ?? 'application/octet-stream' })
    res.end(body)
  } catch (err) {
    res.writeHead(500)
    res.end(`Server error: ${err instanceof Error ? err.message : String(err)}`)
  }
})

server.listen(PORT, () => {
  // eslint-disable-next-line no-console
  console.log(`Faucet static server listening on http://localhost:${PORT} (root: ${ROOT})`)
})
