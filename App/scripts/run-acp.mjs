// Launch the harness engine as an ACP server on this process's stdio.
//
// `make run` must come up listening and stay up: an ACP client (the native app,
// or an evaluation harness) attaches to the running process and drives sessions
// over stdin/stdout. This wrapper owns the two things `dsh` cannot know by
// itself — which workspace the engine serves, and which model route the first
// session starts on:
//
//   * The engine runs with its working directory at the repository root, so the
//     project `.env` layer is the checkout the user launched. The workspace the
//     agent works in travels separately: the client names it in `session/new`
//     `cwd`, and this wrapper reads it from `HARNESS_WORKSPACE` (the app sets it
//     when it opens a different folder) to know whose route document to read.
//   * The route comes from the workspace's `.harness/models.json`'s `active`
//     entry — falling back to the repository's own document, then the
//     composition's default route: AI_MODEL at AI_BASE_URL, OpenRouter's
//     DeepSeek when neither is set (see cordis.yml). So a clean clone with only
//     AI_API_KEY runs; the application or AI_MODEL chooses another model. The
//     provider itself reads the same document — its path travels in the
//     generated overlay, because the plugin resolves a relative one against
//     the engine's working directory, which is always this repository.
//
// The ACP bridge is the profile's application, not a flag: `hackathon-harness`
// composes the ACP bundle, whose app takes no options of its own, reads stdin
// until EOF, and then shuts the process down. Stdout carries ACP and nothing
// else — every diagnostic here goes to stderr.
//
// Usage: node App/scripts/run-acp.mjs [--print]
//   --print  print the engine command and exit without launching it.
// Environment:
//   DSH_HOME           the Harness home (default: <repo>/.dsh)
//   HARNESS_WORKSPACE  the folder the agent serves (default: <repo>)

import { spawn } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const app = resolve(here, '..')
const repo = resolve(app, '..')
const dsh = resolve(repo, 'Deepseek', 'deepseek-harness')

const home = process.env.DSH_HOME?.trim() ? resolve(process.env.DSH_HOME) : join(repo, '.dsh')
const workspace = process.env.HARNESS_WORKSPACE?.trim() ? resolve(process.env.HARNESS_WORKSPACE) : repo
const modelsFiles = [...new Set([join(workspace, '.harness', 'models.json'), join(repo, '.harness', 'models.json')])]

/** The route the composition serves when no models document names one (cordis.yml's defaultRoute). */
const DEFAULT_MODEL = 'deepseek/deepseek-v4-flash'
/** The endpoint of that route. */
const DEFAULT_BASE_URL = 'https://openrouter.ai/api/v1'
/** The working context the default route advertises, which is what compaction works within. */
const DEFAULT_CONTEXT_WINDOW = '131072'
/**
 * Output tokens one response of the default route may use.
 *
 * Deliberately conservative rather than dsh's own 32768 fallback: some
 * providers (Groq is one) reject a request above their own per-model output
 * ceiling even though it is well under the model's context window, and dsh's
 * fallback applies whenever a route's config does not set `maxTokens`
 * explicitly — which the default route never did until this was added, so a
 * route on such a provider failed every turn with "max_completion_tokens must
 * be less than or equal to ...". 2000 is comfortably under every such ceiling
 * seen in practice; raise it with AI_MAX_TOKENS for a route that allows more.
 */
const DEFAULT_MAX_TOKENS = '2000'

/**
 * The harness's own `AI_*` settings from the `.env` files, for the engine's
 * process environment.
 *
 * The composition reads `AI_MODEL`, `AI_BASE_URL`, and `AI_CONTEXT_WINDOW` from
 * `process.env` when it is loaded, which is before any `.env` layer the engine
 * itself applies — so an evaluator who writes the model into `.env` beside
 * `AI_API_KEY` would otherwise get the default model. The workspace's file wins
 * over the repository's, and a variable already exported in the shell wins
 * over both. Only `AI_*` names are taken: this is the harness's settings
 * channel, not a general environment loader.
 * @returns the variables to add to the engine's environment.
 */
function dotenvSettings() {
  const settings = {}
  for (const dir of [...new Set([repo, workspace])]) {
    const path = join(dir, '.env')
    if (!existsSync(path)) continue
    for (const raw of readFileSync(path, 'utf8').split(/\r?\n/u)) {
      const line = raw.trim()
      if (line.length === 0 || line.startsWith('#')) continue
      const match = /^(?:export\s+)?(AI_[A-Z0-9_]*)\s*=\s*(.*)$/u.exec(line)
      if (match === null) continue
      let value = match[2].trim()
      if (value.length >= 2 && (value[0] === '"' || value[0] === "'") && value.at(-1) === value[0]) {
        value = value.slice(1, -1)
      }
      settings[match[1]] = value
    }
  }
  for (const name of Object.keys(settings)) {
    if (process.env[name]?.trim()) delete settings[name]
  }
  return settings
}

const settings = dotenvSettings()
/** One harness setting, from the shell first and the `.env` files second. */
const setting = name => process.env[name]?.trim() || settings[name]?.trim() || undefined

/**
 * The default route, resolved once here and handed to the engine as plain
 * environment: the overlay below has to restate the provider row's whole
 * config, and resolving the values in one place keeps the row the overlay
 * writes and the route the `acp` row names from ever disagreeing.
 */
const route = {
  AI_MODEL: setting('AI_MODEL') ?? setting('AI_HARNESS_FALLBACK_MODEL') ?? DEFAULT_MODEL,
  AI_BASE_URL: setting('AI_BASE_URL') ?? setting('AI_HARNESS_FALLBACK_BASE_URL') ?? DEFAULT_BASE_URL,
  AI_CONTEXT_WINDOW: setting('AI_CONTEXT_WINDOW') ?? DEFAULT_CONTEXT_WINDOW,
  AI_MAX_TOKENS: setting('AI_MAX_TOKENS') ?? DEFAULT_MAX_TOKENS,
}

/**
 * The route the next session starts on.
 *
 * The acp row ships a default provider this composition disables, so every
 * launch has to name one explicitly. The workspace's document's `active` entry
 * wins; with none, the repository's document; then the composition's default
 * route, which the provider serves under the `default` key.
 * @returns the chosen provider and model.
 */
function selection() {
  for (const path of modelsFiles) {
    if (!existsSync(path)) continue
    let document
    try {
      document = JSON.parse(readFileSync(path, 'utf8'))
    } catch (error) {
      process.stderr.write(`run-acp: cannot read ${path}: ${String(error)}\n`)
      process.exit(1)
    }
    const active = document?.active
    if (active !== undefined && typeof active.provider === 'string' && typeof active.model === 'string') {
      return { provider: active.provider, model: active.model, placeholder: false }
    }
  }
  // cordis.yml always declares the default route, under the `default` key:
  // AI_MODEL (or the older fallback name) when set, OpenRouter's DeepSeek
  // otherwise. The model named here has to be the one that row serves.
  return { provider: 'default', model: route.AI_MODEL, placeholder: false }
}

/**
 * The models document the provider should read.
 *
 * The provider resolves its `modelsFile` against the engine's working directory,
 * which is the repository even when the agent serves another folder — so the
 * folder's own document has to be named outright, or a route added while working
 * somewhere else would never reach a request. The workspace's document wins; the
 * repository's is the fallback, and the default path when neither exists.
 * @returns the document path to hand the provider.
 */
function modelsFile() {
  return modelsFiles.find(existsSync) ?? modelsFiles[0]
}

/**
 * Writes the overlay that hands the engine the chosen route: the acp row's
 * shipped default names a route this composition disables, so the selected one
 * has to arrive per launch.
 * @param choice the route {@link selection} chose
 * @returns the overlay patch path.
 */
function selectionPatch(choice) {
  const dir = join(home, 'launch')
  mkdirSync(dir, { recursive: true })
  const path = join(dir, `selection-${process.pid}.patch.yml`)
  writeFileSync(path, [
    '# Generated by App/scripts/run-acp.mjs — the route the next session starts on.',
    '# Edit .harness/models.json (or add a model in the application), not this file:',
    '# it is rewritten on every launch.',
    '- id: acp',
    "  name: '@deepseek-ai/dsh-acp'",
    '  config:',
    `    provider: ${JSON.stringify(choice.provider)}`,
    `    model: ${JSON.stringify(choice.model)}`,
    '',
    '# The route source follows the workspace, not the launch directory. A config',
    "# override replaces the row's whole config, so every field is restated here —",
    '# an overlay that named only modelsFile silently dropped the default route.',
    '- id: llm-harness',
    '  config:',
    `    modelsFile: ${JSON.stringify(modelsFile())}`,
    '    apiKeyEnv: AI_API_KEY',
    '    defaultRoute: !!js "({ displayName: process.env.AI_MODEL, baseURL: process.env.AI_BASE_URL,'
      + ' model: process.env.AI_MODEL, contextWindow: Number(process.env.AI_CONTEXT_WINDOW),'
      + ' maxTokens: Number(process.env.AI_MAX_TOKENS) })"',
    '',
  ].join('\n'))
  return path
}

const choice = selection()
const patch = selectionPatch(choice)
process.on('exit', () => { rmSync(patch, { force: true }) })
if (choice.provider === 'default') {
  process.stderr.write(`run-acp: model ${choice.model} at ${route.AI_BASE_URL} (set AI_MODEL / AI_BASE_URL to change it)\n`)
}
if (!setting('AI_API_KEY')) {
  process.stderr.write('run-acp: AI_API_KEY is not set (environment or .env); every prompt will fail until it is\n')
}

const bin = join(dsh, 'apps', 'cli', 'src', 'bin.ts')
const loader = join(dsh, 'node_modules', 'tsx', 'dist', 'loader.mjs')
if (!existsSync(bin) || !existsSync(loader)) {
  process.stderr.write(`run-acp: the dsh checkout is not installed at ${dsh}; run 'make setup' first\n`)
  process.exit(1)
}

const args = [
  '--import', loader,
  bin,
  '--profile', 'hackathon-harness',
  '--patch', patch,
]

if (process.argv.includes('--print')) {
  process.stdout.write(`DSH_HOME=${home} cwd=${repo} workspace=${workspace} node ${args.join(' ')}\n`)
  process.exit(0)
}

process.stderr.write(`run-acp: profile hackathon-harness, workspace ${workspace}, DSH_HOME ${home}\n`)
process.stderr.write('run-acp: the engine is up and this terminal now serves ACP on stdin/stdout;'
  + " Ctrl+C stops it. The windowed application is 'make app' (or 'make app-release') in another"
  + ' terminal — it opens its own engine, so this one is only needed to serve an external ACP client.\n')
const child = spawn(process.execPath, args, {
  cwd: repo,
  env: {
    ...process.env,
    // The dsh tree is TypeScript run from source; its own tsconfig is what
    // resolves the `@deepseek-ai/*` path facade for the plugins it loads.
    TSX_TSCONFIG_PATH: join(dsh, 'tsconfig.json'),
    DSH_HOME: home,
    ...settings,
    ...route,
  },
  stdio: 'inherit',
})

// Signals reach a process group, not always the child: forward them so a
// stopped `make run` stops the engine. The launcher then exits with the
// engine's own status, so an evaluation harness sees the engine's outcome and
// not this wrapper's.
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => { child.kill(signal) })
}
child.on('error', error => {
  process.stderr.write(`run-acp: engine launch failed: ${error.message}\n`)
  process.exit(1)
})
child.on('exit', (code, signal) => {
  if (signal !== null) {
    process.kill(process.pid, signal)
    return
  }
  process.exit(code ?? 0)
})
