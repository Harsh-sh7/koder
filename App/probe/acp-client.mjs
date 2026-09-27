// ACP client used by the repository's probes: it boots the harness engine for
// one profile over a pipe, speaks NDJSON JSON-RPC, and answers the one request
// the agent makes back to a client (tool permission). It exists so a probe can
// assert on a real engine instead of a stub.

import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { dirname, join, resolve } from 'node:path'
import { readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'

const here = dirname(fileURLToPath(import.meta.url))

/** Repository root of the dsh checkout this app runs on. */
export const dshRepo = process.env.DSH_REPO ?? resolve(here, '../../Deepseek/deepseek-harness')

/**
 * Write the patch overlay that gives a session its initial route.
 *
 * The profile ships no model, so the engine starts with no selection at all;
 * the application is what names one, from the document it wrote. Without this
 * the session exists but has no model and no picker to choose one from.
 * @param workspace - workspace root holding `.harness/models.json`.
 * @returns the patch path, or undefined when the document names no active route.
 */
export function writeSelectionPatch(workspace) {
  let document
  try {
    document = JSON.parse(readFileSync(join(workspace, '.harness/models.json'), 'utf8'))
  } catch {
    return undefined
  }
  if (document.active === undefined) return undefined
  const path = join(tmpdir(), `harness-active-${process.pid}-${Date.now()}.yml`)
  writeFileSync(path, [
    '# Generated from .harness/models.json — the route a new session starts on.',
    '- id: acp',
    "  name: '@deepseek-ai/dsh-acp'",
    '  config:',
    `    provider: ${JSON.stringify(document.active.provider)}`,
    `    model: ${JSON.stringify(document.active.model)}`,
    '',
  ].join('\n'))
  return path
}

/**
 * Boot one engine and return a small ACP client over it.
 * @param options - workspace, profile, optional patch overlay, and echo flag.
 * @returns the client, its notification log, and a close operation.
 */
export function bootEngine(options) {
  const workspace = options.workspace
  const patch = options.patch ?? writeSelectionPatch(workspace)
  const args = [
    '--import', resolve(dshRepo, 'node_modules/tsx/dist/loader.mjs'),
    resolve(dshRepo, 'apps/cli/src/bin.ts'),
    '--profile', options.profile ?? 'harness',
    ...patch === undefined ? [] : ['--patch', patch],
  ]
  const child = spawn(process.execPath, options.launcher === undefined ? args : [options.launcher], {
    cwd: workspace,
    env: {
      ...process.env,
      TSX_TSCONFIG_PATH: resolve(dshRepo, 'tsconfig.json'),
      DSH_HOME: process.env.DSH_HOME ?? resolve(workspace, '.dsh'),
      ...options.env,
      HARNESS_WORKSPACE: workspace,
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })

  const pending = new Map()
  const notifications = []
  let nextId = 1
  const echo = options.echo === true

  const send = (method, params) => {
    const id = nextId++
    const message = { jsonrpc: '2.0', id, method, params }
    if (echo) process.stdout.write(`--> ${JSON.stringify(message)}\n`)
    child.stdin.write(`${JSON.stringify(message)}\n`)
    return new Promise((resolvePromise, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id)
        reject(new Error(`${method}: timed out`))
      }, options.timeoutMs ?? 90_000)
      pending.set(id, {
        resolve: value => { clearTimeout(timer); resolvePromise(value) },
        reject: error => { clearTimeout(timer); reject(error) },
        method,
      })
    })
  }

  const respond = (id, result) => {
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, result })}\n`)
  }

  createInterface({ input: child.stdout }).on('line', (line) => {
    if (echo) process.stdout.write(`<-- ${line}\n`)
    let message
    try {
      message = JSON.parse(line)
    } catch {
      return
    }
    if (message.method !== undefined && message.id !== undefined) {
      if (message.method === 'session/request_permission') {
        respond(message.id, { outcome: { outcome: 'selected', optionId: 'allow-once' } })
      } else {
        respond(message.id, {})
      }
      return
    }
    if (message.method !== undefined) {
      notifications.push(message)
      return
    }
    if (message.id === undefined) return
    const waiter = pending.get(message.id)
    if (waiter === undefined) return
    pending.delete(message.id)
    if (message.error !== undefined) waiter.reject(new Error(`${waiter.method}: ${JSON.stringify(message.error)}`))
    else waiter.resolve(message.result)
  })

  const exited = new Promise(resolve => child.once('exit', code => {
    for (const waiter of pending.values()) waiter.reject(new Error(`engine exited (${code}) during ${waiter.method}`))
    pending.clear()
    resolve(code)
  }))
  child.on('error', error => {
    for (const waiter of pending.values()) waiter.reject(error)
    pending.clear()
  })
  const stderr = []
  child.stderr.on('data', (chunk) => {
    stderr.push(String(chunk))
    if (echo) process.stderr.write(`[stderr] ${chunk}`)
  })

  return {
    child,
    send,
    notifications,
    stderr,
    /** Session updates for one session id, in arrival order. */
    updatesFor: (sessionId) => notifications
      .filter(message => message.method === 'session/update' && message.params.sessionId === sessionId)
      .map(message => message.params.update),
    async close() {
      child.stdin.end()
      const timer = setTimeout(() => { child.kill('SIGTERM') }, 8_000)
      try { return await exited } finally { clearTimeout(timer) }
    },
  }
}
