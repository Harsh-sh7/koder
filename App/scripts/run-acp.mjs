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
//     entry — falling back to the repository's own document, the deployment
//     fallback (`AI_HARNESS_FALLBACK_MODEL`/`_BASE_URL`), or the provider's
//     built-in placeholder route, in that order. The composition ships no model,
//     so an unconfigured workspace opens sessions on the placeholder (and
//     prompts fail until one is added); the wrapper says so on stderr and starts
//     anyway. The provider itself reads the same document — its path travels in
//     the generated overlay, because the plugin resolves a relative one against
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
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const app = resolve(here, '..')
const repo = resolve(app, '..')
const dsh = resolve(repo, 'Deepseek', 'deepseek-harness')

const home = process.env.DSH_HOME?.trim() ? resolve(process.env.DSH_HOME) : join(repo, '.dsh')
const workspace = process.env.HARNESS_WORKSPACE?.trim() ? resolve(process.env.HARNESS_WORKSPACE) : repo
const modelsFiles = [...new Set([join(workspace, '.harness', 'models.json'), join(repo, '.harness', 'models.json')])]

/**
 * The route the next session starts on.
 *
 * The acp row ships a default provider this composition disables, so every
 * launch has to name one explicitly. The workspace's document's `active` entry
 * wins; with none, the repository's document; then the deployment fallback
 * (which the provider serves under the `default` key); then the provider's
 * built-in placeholder route — a session, and only a session, can be opened on
 * it, which is what an unconfigured workspace gets.
 * @returns the chosen provider and model.
 */
function selection() {
  for (const modelsFile of modelsFiles) {
    if (!existsSync(modelsFile)) continue
    let document
    try {
      document = JSON.parse(readFileSync(modelsFile, 'utf8'))
    } catch (error) {
      process.stderr.write(`run-acp: cannot read ${modelsFile}: ${String(error)}\n`)
      continue
    }
    const active = document?.active
    if (active !== undefined && typeof active.provider === 'string' && typeof active.model === 'string') {
      return { provider: active.provider, model: active.model, placeholder: false }
    }
  }
  const fallbackModel = process.env.AI_HARNESS_FALLBACK_MODEL?.trim()
  const fallbackBaseURL = process.env.AI_HARNESS_FALLBACK_BASE_URL?.trim()
  if (fallbackModel && fallbackBaseURL) return { provider: 'default', model: fallbackModel, placeholder: false }
  return { provider: 'unconfigured', model: 'unconfigured', placeholder: true }
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
  return modelsFiles.find(existsSync) ?? modelsFiles[modelsFiles.length - 1]
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
  const path = join(dir, 'selection.patch.yml')
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
    '# The route source follows the workspace, not the launch directory.',
    '- id: llm-harness',
    '  config:',
    `    modelsFile: ${JSON.stringify(modelsFile())}`,
    '',
  ].join('\n'))
  return path
}

const choice = selection()
const patch = selectionPatch(choice)
if (choice.placeholder) {
  process.stderr.write('run-acp: no active model in .harness/models.json; sessions open on the built-in'
    + ' placeholder route and prompts will fail until one is added (the application adds one, or set'
    + ' AI_HARNESS_FALLBACK_MODEL and AI_HARNESS_FALLBACK_BASE_URL in .env)\n')
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
child.on('exit', (code, signal) => {
  if (signal !== null) {
    process.kill(process.pid, signal)
    return
  }
  process.exit(code ?? 0)
})
