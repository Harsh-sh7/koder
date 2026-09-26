// Proves the property the application depends on: a model added to the models
// document while the engine is already running reaches the *next* request.
//
// The probe creates a session, adds a second route to the document, waits past
// the plugin's re-read interval, and creates a second session. The second
// session's model options must contain both routes while the first session's
// engine process never restarted.
//
// Usage: node live-routes.mjs

import { readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { dirname } from 'node:path'
import { bootEngine } from './acp-client.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const workspace = process.env.WORKSPACE ?? resolve(here, '../..')
const documentPath = join(workspace, '.harness/models.json')
const original = readFileSync(documentPath, 'utf8')

const engine = bootEngine({ workspace })

const fail = async (message) => {
  process.stderr.write(`FAIL: ${message}\n`)
  await engine.close()
  writeFileSync(documentPath, original)
  process.exit(1)
}

try {
  await engine.send('initialize', { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: 'live-routes', version: '0.0.1' } })
  const first = await engine.send('session/new', { cwd: workspace, mcpServers: [] })
  const firstRoutes = routeNames(first.configOptions)
  process.stdout.write(`first session routes: ${JSON.stringify(firstRoutes)}\n`)

  const document = JSON.parse(original)
  document.providers.second = {
    displayName: 'Second',
    api: 'openai-completions',
    baseURL: 'http://127.0.0.1:8899/v1',
    apiKeyEnv: 'AI_API_KEY',
    models: [{ id: 'second-model', name: 'second-model' }],
  }
  writeFileSync(documentPath, `${JSON.stringify(document, undefined, 2)}\n`)
  process.stdout.write('document: added route "second"\n')

  // Past the plugin's re-read interval, with no engine restart in between.
  await new Promise(resolvePromise => setTimeout(resolvePromise, 3000))

  const second = await engine.send('session/new', { cwd: workspace, mcpServers: [] })
  const secondRoutes = routeNames(second.configOptions)
  process.stdout.write(`second session routes: ${JSON.stringify(secondRoutes)}\n`)

  if (firstRoutes.includes('second')) await fail('the first session already saw the added route; the wait proved nothing')
  if (!secondRoutes.includes('second')) await fail('the added route never reached the running engine')

  process.stdout.write('OK: the added route reached the engine without a restart\n')
} finally {
  writeFileSync(documentPath, original)
  await engine.close()
}

/** Provider ids the model picker offers for one session. */
function routeNames(configOptions) {
  const model = (configOptions ?? []).find(option => option.id === 'model')
  return (model?.options ?? []).map(group => group.group)
}
