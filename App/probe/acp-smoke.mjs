// ACP smoke client: boots the harness engine for one profile, speaks the
// smallest real ACP session over its stdio, and prints every line it exchanges.
//
// Usage: node acp-smoke.mjs [--prompt <text>] [--prompt2 <text>] [--patch <path> ...]
//   --prompt2  send a second prompt in the same session (a resumed-turn probe)
//   --patch    extra composition overlay applied on top of the generated one (repeatable)
// Environment: DSH_HOME, WORKSPACE (defaults to the repository root), DSH_REPO.
//   SMOKE_ENGINE  launch this command instead of the engine directly, so the same
//                 handshake proves a launcher (`SMOKE_ENGINE="make run"`).

import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const repo = process.env.DSH_REPO ?? resolve(here, '../../Deepseek/deepseek-harness')
const workspace = process.env.WORKSPACE ?? resolve(here, '../..')

const argv = process.argv.slice(2)
const option = (name) => {
  const index = argv.indexOf(name)
  return index === -1 ? undefined : argv[index + 1]
}
const prompt = option('--prompt')
const prompt2 = option('--prompt2')
/** Every `--patch <path>` overlay, in the order given, resolved against this directory. */
const extraPatches = argv.flatMap((value, index) => value === '--patch' ? [resolve(here, argv[index + 1])] : [])

// The application picks the session's initial route from the models document it
// writes, and hands it to the engine as a patch overlay: the profile ships no
// model, so nothing runs until one is chosen. With no selection the launcher's
// own rule applies (App/scripts/run-acp.mjs): the deployment fallback when it is
// declared, otherwise the provider's built-in placeholder route — which still
// lets this probe open a session on a workspace nobody has configured.
function selectionPatch() {
  let document
  try {
    document = JSON.parse(readFileSync(join(workspace, '.harness/models.json'), 'utf8'))
  } catch {
    document = undefined
  }
  const active = document?.active
  const fallbackModel = process.env.AI_HARNESS_FALLBACK_MODEL?.trim()
  const fallbackBaseURL = process.env.AI_HARNESS_FALLBACK_BASE_URL?.trim()
  const choice = active !== undefined
    ? { provider: active.provider, model: active.model }
    : fallbackModel && fallbackBaseURL
      ? { provider: 'default', model: fallbackModel }
      : { provider: 'unconfigured', model: 'unconfigured' }
  const path = join(tmpdir(), `harness-active-${process.pid}.yml`)
  writeFileSync(path, [
    `# Generated from .harness/models.json — the route the session starts on.`,
    `- id: acp`,
    `  name: '@deepseek-ai/dsh-acp'`,
    `  config:`,
    `    provider: ${JSON.stringify(choice.provider)}`,
    `    model: ${JSON.stringify(choice.model)}`,
    ``,
  ].join('\n'))
  return path
}

const patch = selectionPatch()

// The engine, or whatever launcher is under test. A launcher owns its own cwd,
// environment, profile, and overlays, so nothing from above is passed to it.
const engineCommand = process.env.SMOKE_ENGINE?.trim()
const child = engineCommand === undefined || engineCommand === ''
  ? spawn(
    process.execPath,
    [
      '--import', resolve(repo, 'node_modules/tsx/dist/loader.mjs'),
      resolve(repo, 'apps/cli/src/bin.ts'),
      '--profile', 'hackathon-harness',
      '--patch', patch,
      ...extraPatches.flatMap(path => path === undefined ? [] : ['--patch', path]),
    ],
    {
      cwd: workspace,
      env: {
        ...process.env,
        TSX_TSCONFIG_PATH: resolve(repo, 'tsconfig.json'),
        DSH_HOME: process.env.DSH_HOME ?? resolve(workspace, '.dsh'),
      },
      stdio: ['pipe', 'pipe', 'pipe'],
    },
  )
  : spawn(engineCommand, { cwd: workspace, shell: true, stdio: ['pipe', 'pipe', 'pipe'] })

let nextId = 1
const pending = new Map()

function send(method, params) {
  const id = nextId++
  const message = { jsonrpc: '2.0', id, method, params }
  process.stdout.write(`--> ${JSON.stringify(message)}\n`)
  child.stdin.write(`${JSON.stringify(message)}\n`)
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject, method }))
}

function respond(id, result) {
  const message = { jsonrpc: '2.0', id, result }
  process.stdout.write(`--> ${JSON.stringify(message)}\n`)
  child.stdin.write(`${JSON.stringify(message)}\n`)
}

/** Every `session/update` the engine published, across all prompts. */
const events = []

createInterface({ input: child.stdout }).on('line', (line) => {
  process.stdout.write(`<-- ${line}\n`)
  let message
  try {
    message = JSON.parse(line)
  } catch {
    return
  }
  if (message.method === 'session/update') events.push(message.params.update)
  if (message.method !== undefined && message.id !== undefined) {
    // Agent-to-client request. Permission is the only one this smoke client
    // must answer; everything else is reported and declined.
    if (message.method === 'session/request_permission') {
      respond(message.id, { outcome: { outcome: 'selected', optionId: 'allow-once' } })
    } else {
      respond(message.id, {})
    }
    return
  }
  if (message.id === undefined) return
  const waiter = pending.get(message.id)
  if (waiter === undefined) return
  pending.delete(message.id)
  if (message.error !== undefined) waiter.reject(new Error(`${waiter.method}: ${JSON.stringify(message.error)}`))
  else waiter.resolve(message.result)
})

child.stderr.on('data', (chunk) => {
  process.stderr.write(`[stderr] ${chunk}`)
})

const created = await send('initialize', {
  protocolVersion: 1,
  clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } },
  clientInfo: { name: 'acp-smoke', version: '0.0.1' },
})
process.stdout.write(`initialize: ${JSON.stringify(created, undefined, 2)}\n`)

const session = await send('session/new', { cwd: workspace, mcpServers: [] })
process.stdout.write(`session/new: ${JSON.stringify(session, undefined, 2)}\n`)

for (const [index, text] of [prompt, prompt2].entries()) {
  if (text === undefined) continue
  const from = events.length
  const result = await send('session/prompt', { sessionId: session.sessionId, prompt: [{ type: 'text', text }] })
  process.stdout.write(`session/prompt ${index + 1}: ${JSON.stringify(result)}\n`)
  process.stdout.write(`updates ${index + 1}: ${JSON.stringify(events.slice(from), undefined, 2)}\n`)
}

child.stdin.end()
const exit = await new Promise(resolvePromise => child.once('exit', code => resolvePromise(code)))
process.stdout.write(`engine exit: ${exit}\n`)
