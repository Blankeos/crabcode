import { createServer } from 'node:http'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { extname, resolve, sep } from 'node:path'

export async function startStaticServer(root: string) {
  const absoluteRoot = resolve(root)
  let lastError: Error | null = null

  for (let attempt = 0; attempt < 20; attempt++) {
    const port = 41_000 + Math.floor(Math.random() * 20_000)
    try {
      return await listenStaticServer(absoluteRoot, port)
    } catch (err) {
      lastError = err instanceof Error ? err : new Error(String(err))
    }
  }

  throw lastError ?? new Error('failed to start static server')
}

function listenStaticServer(absoluteRoot: string, port: number) {
  const server = createServer((request, response) => {
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      response.writeHead(405, { allow: 'GET, HEAD' })
      response.end('Method not allowed')
      return
    }

    let requestPath = 'index.html'
    try {
      const url = new URL(request.url ?? '/', 'http://127.0.0.1')
      requestPath = decodeURIComponent(url.pathname).replace(/^\/+/, '') || 'index.html'
    } catch {
      response.writeHead(400)
      response.end('Bad request')
      return
    }

    const filePath = resolve(absoluteRoot, requestPath)
    if (filePath !== absoluteRoot && !filePath.startsWith(absoluteRoot + sep)) {
      response.writeHead(403)
      response.end('Forbidden')
      return
    }

    if (!existsSync(filePath) || statSync(filePath).isDirectory()) {
      response.writeHead(404)
      response.end('Not found')
      return
    }

    response.writeHead(200, { 'content-type': contentTypeFor(filePath) })
    if (request.method === 'HEAD') {
      response.end()
      return
    }
    response.end(readFileSync(filePath))
  })

  return new Promise<{ url: string; close: () => Promise<void> }>((resolveStart, rejectStart) => {
    let settled = false
    const onError = (err: Error) => {
      if (settled) return
      settled = true
      rejectStart(err)
    }
    server.once('error', onError)
    try {
      server.listen(port, '127.0.0.1', () => {
        if (settled) return
        settled = true
        server.off('error', onError)
        resolveStart({
          url: `http://127.0.0.1:${port}`,
          close: () =>
            new Promise((resolveClose) => {
              server.close(() => resolveClose())
            }),
        })
      })
    } catch (err) {
      onError(err instanceof Error ? err : new Error(String(err)))
    }
  })
}

function contentTypeFor(path: string) {
  switch (extname(path)) {
    case '.json':
      return 'application/json; charset=utf-8'
    case '.md':
      return 'text/markdown; charset=utf-8'
    case '.txt':
      return 'text/plain; charset=utf-8'
    default:
      return 'application/octet-stream'
  }
}

