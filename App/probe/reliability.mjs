// Offline regression through the real launcher, ACP engine, provider and children.
// Run with make check-reliability after make setup. No live credentials are used.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { bootEngine } from './acp-client.mjs'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const workspace = mkdtempSync(join(tmpdir(), 'harness-reliability-'))
const recording = join(workspace, 'requests.jsonl')
const mock = spawn(process.execPath, [join(repo, 'App/probe/mock-openai.mjs'), '--build', '--port', '0', '--record', recording], { stdio: ['ignore', 'pipe', 'pipe'] })
let engine
try {
  const endpoint = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('mock startup timed out')), 10_000)
    mock.once('error', reject)
    mock.stdout.on('data', chunk => {
      const hit = String(chunk).match(/http:\/\/127\.0\.0\.1:\d+\/v1/u)
      if (hit) { clearTimeout(timer); resolve(hit[0]) }
    })
  })
  mkdirSync(join(workspace, '.harness'))
  const routes = { providers: { mock: {
    api: 'openai-completions', baseURL: endpoint, apiKeyEnv: 'AI_API_KEY',
    models: [{ id: 'model-a' }, { id: 'model-b' }],
  } }, active: { provider: 'mock', model: 'model-a' } }
  writeFileSync(join(workspace, '.harness/models.json'), JSON.stringify(routes))
  writeFileSync(join(workspace, '.env'), 'AI_API_KEY=mock-key\n')
  const boot = () => bootEngine({ workspace, launcher: join(repo, 'App/scripts/run-acp.mjs'), env: {
    DSH_HOME: join(repo, '.dsh'), AI_API_KEY: '',
    AI_BASE_URL: endpoint, AI_MODEL: 'model-a',
  } })
  const init = client => client.send('initialize', { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: 'reliability', version: '1' } })
  const records = () => readFileSync(recording, 'utf8').trim().split('\n').map(line => JSON.parse(line))
  engine = boot()
  await init(engine)
  const session = await engine.send('session/new', { cwd: workspace, mcpServers: [] })
  const original = 'Build the three independent fixture parts using run_wave. Remember task marker RELIABILITY-ORIGINAL.'
  await engine.send('session/prompt', { sessionId: session.sessionId, prompt: [{ type: 'text', text: original }] })
  const updates = JSON.stringify(engine.updatesFor(session.sessionId))
  assert.match(updates, /3 of 3 parts done/u)
  for (const [part, file] of [['core', 'todo.py'], ['tests', 'test_todo.py'], ['docs', 'README.md']]) {
    assert.match(readFileSync(join(workspace, '.harness/ui-check', file), 'utf8'), new RegExp(`built by the ${part} part`, 'u'))
  }
  const childRequests = records().filter(row => row.body.tools?.some(tool => tool.function.name === 'structured_output'))
  assert.ok(childRequests.length >= 6, 'three children must each write and return structured output')
  assert.ok(childRequests.every(row => row.body.model === 'model-a'), 'children inherit the selected model')
  console.log('PASS: real parallel children write their own fixture files and return results over ACP')
  await engine.close()
  engine = undefined

  routes.active.model = 'model-b'
  writeFileSync(join(workspace, '.harness/models.json'), JSON.stringify(routes))
  writeFileSync(join(workspace, '.env'), 'AI_API_KEY=mock-key-replaced\n')
  engine = boot()
  await init(engine)
  const resumed = await engine.send('session/resume', { sessionId: session.sessionId, cwd: workspace, mcpServers: [] })
  const option = resumed.configOptions.find(option => option.id === 'model')
  const choices = option.options.flatMap(item => item.options ?? [item])
  const next = choices.find(item => { try { return JSON.stringify(JSON.parse(item.value)) === JSON.stringify(['mock', 'model-b']) } catch { return false } })
  assert.ok(next, 'the new route must be advertised after restart')
  await engine.send('session/set_config_option', { sessionId: session.sessionId, configId: 'model', value: next.value })
  await engine.send('session/prompt', { sessionId: session.sessionId, prompt: [{ type: 'text', text: 'continue' }] })
  const last = records().at(-1)
  assert.equal(last.body.model, 'model-b')
  assert.ok(JSON.stringify(last.body.messages).includes('RELIABILITY-ORIGINAL'), 'continue must retain the original task context')
  assert.equal(last.credentialHash, createHash('sha256').update('Bearer mock-key-replaced').digest('hex'))
  console.log('PASS: restart/resume retains context and sends the new model and replacement workspace key')
} finally {
  if (engine) await engine.close()
  mock.kill('SIGTERM')
  rmSync(workspace, { recursive: true, force: true })
}
