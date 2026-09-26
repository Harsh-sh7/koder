// Materialize the `hackathon-harness` dsh profile into the application's DSH home.
//
// dsh resolves `--profile <name>` to `$DSH_HOME/profiles/<name>/package.json`, and
// that manifest is a package project of its own: it declares which bundles the
// tree composes and which out-of-tree plugin packages the loader may import. A
// committed profile cannot carry that as-is — the plugin packages live beside it
// in this repository, at absolute paths only this machine knows — so this script
// writes the manifest at setup time with `link:` dependencies resolved against
// the checkout it is running from, then installs them.
//
// It is idempotent and it owns exactly three things in the profile directory:
// our `link:` dependencies, the bundle list, and the two scaffold files dsh
// itself initializes (an empty user patch layer and the pnpm settings an
// out-of-tree plugin needs). A dependency somebody else added stays: `.env`,
// `.harness/`, and `$DSH_HOME` are machine state, and so is a profile.
//
// Usage: node App/scripts/init-profile.mjs
// Environment: DSH_HOME selects the Harness home (default: <repo>/.dsh).

import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const app = resolve(here, '..')
const repo = resolve(app, '..')

const home = process.env.DSH_HOME?.trim() ? resolve(process.env.DSH_HOME) : join(repo, '.dsh')
const profileDir = join(home, 'profiles', 'hackathon-harness')

/** The plugin packages the profile links, in the order they are written. */
const PLUGINS = [
  { name: '@harness/dsh-hackathon-harness-preset', dir: join(app, 'dsh-plugins', 'hackathon-harness-preset') },
  { name: '@harness/dsh-llm-harness-provider', dir: join(app, 'dsh-plugins', 'llm-harness-provider') },
  { name: '@harness/dsh-spec-lock', dir: join(app, 'dsh-plugins', 'spec-lock') },
]

/**
 * The bundle layers the profile composes, in application order. The harness
 * bundle is last because it is the overlay: it turns the shipped routes off and
 * mounts this application's provider, spec, and orchestration rows over the
 * base and ACP layers rather than beside them.
 */
const BUNDLES = [
  '@deepseek-ai/dsh-base',
  '@deepseek-ai/dsh-acp-app',
  '@harness/dsh-hackathon-harness-preset',
]

// The hoisted linker is what makes an out-of-tree plugin's own imports resolve:
// the profile's node_modules holds the plugin package, and the plugin's own
// `node_modules` (from the App workspace install) holds its peers. Matching
// dsh's own profile scaffold keeps the two indistinguishable to Node.
const PNPM_WORKSPACE = `packages:
  - .

nodeLinker: hoisted
autoInstallPeers: false
`

const USER_PATCH = `# This profile's own layer, applied after every bundle layer. Empty on purpose:
# the composition this application runs is the harness-preset bundle's
# cordis.yml, so a row belongs here only when this machine or this profile must
# diverge from that bundle — see App/dsh-plugins/hackathon-harness-preset/cordis.yml.
[]
`

/** One JSON file, written the way every other manifest in this repository writes them. */
function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, undefined, 2)}\n`)
}

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'))
}

mkdirSync(profileDir, { recursive: true })

// Dependencies are merged rather than replaced: a person may have added a
// plugin with `dsh plugin --profile hackathon-harness add`, and re-running setup
// must not remove it.
const manifestPath = join(profileDir, 'package.json')
const previous = existsSync(manifestPath) ? readJson(manifestPath) : {}
const dependencies = { ...(previous.dependencies ?? {}) }
for (const plugin of PLUGINS) dependencies[plugin.name] = `link:${plugin.dir}`
writeJson(manifestPath, {
  name: 'dsh-profile-hackathon-harness',
  private: true,
  dependencies,
  dsh: { profile: { bundles: BUNDLES } },
})

const workspacePath = join(profileDir, 'pnpm-workspace.yaml')
if (!existsSync(workspacePath)) writeFileSync(workspacePath, PNPM_WORKSPACE)
const patchPath = join(profileDir, 'cordis.patch.yml')
if (!existsSync(patchPath)) writeFileSync(patchPath, USER_PATCH)

for (const plugin of PLUGINS) {
  if (!existsSync(join(plugin.dir, 'package.json'))) {
    process.stderr.write(`init-profile: missing plugin package ${plugin.dir}\n`)
    process.exit(1)
  }
}

process.stdout.write(`init-profile: installing ${String(PLUGINS.length)} linked plugins in ${profileDir}\n`)
const install = spawnSync('pnpm', ['install'], { cwd: profileDir, stdio: 'inherit' })
if (install.error !== undefined) {
  process.stderr.write(`init-profile: pnpm install failed: ${install.error.message}\n`)
  process.exit(1)
}
if (install.status !== 0) process.exit(install.status ?? 1)

process.stdout.write(`init-profile: profile hackathon-harness ready (DSH_HOME=${home})\n`)
