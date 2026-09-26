/**
 * AI Harness model routes: OpenAI-compatible providers for the harness, read
 * from the workspace models document the application writes, plus one optional
 * fallback route declared in `cordis.yml`. When neither declares a route the
 * plugin still serves one placeholder route, so `session/new` — which names a
 * route — keeps working on a workspace nobody has configured yet.
 *
 * The protocol work is pi-ai's: this plugin owns the route *source* and the
 * credential boundary, and hands both to a `PiAiAdapter` through
 * `resolveProfiles`, so a route here means exactly what the same profile means
 * in `llm-pi-ai`'s own configuration. What differs is where the profile comes
 * from and when it takes effect: the document is re-read whenever its bytes
 * change, and the adapter's route set is replaced in place, so adding a model
 * in the application reaches the next request without a restart.
 *
 * Credentials stay inside the workspace: every route resolves `AI_API_KEY`
 * (or the reference it names) through the credentials seam, then the launch
 * environment, and a miss is a `MISSING_CREDENTIAL` failure rather than a
 * silent fallback to a provider-native ambient key — the harness bills the key
 * the workspace declares, never one it happened to find.
 *
 * ```yaml
 * - id: llm-harness
 *   name: '@harness/dsh-llm-harness-provider'
 *   config:
 *     modelsFile: .harness/models.json
 *     apiKeyEnv: AI_API_KEY
 *     defaultRoute:
 *       displayName: DeepSeek
 *       baseURL: https://api.deepseek.com/v1
 *       model: deepseek-chat
 * ```
 *
 * @module @harness/dsh-llm-harness-provider
 */

import { isAbsolute, resolve } from 'node:path'
import { credentialRef, type CredentialRef } from '@deepseek-ai/dsh-credentials'
import type { Context } from '@deepseek-ai/cordis'
import { assertUsableApiKey, LlmError } from '@deepseek-ai/dsh-llm'
import type { AdapterRegistrationHandle } from '@deepseek-ai/dsh-llm'
import { launchEnvironmentOf } from '@deepseek-ai/dsh-launch-environment'
import { PiAiAdapter } from '@deepseek-ai/dsh-llm-pi-ai'
import type { PiAiProviderProfile, ResolvedPiAiProviderProfile } from '@deepseek-ai/dsh-llm-pi-ai'
// `resolveProfiles` is the adapter's own profile resolver, published through
// the package's `./src/*` export rather than its root: re-deriving it here
// would be re-implementing catalog resolution, and a route that resolved
// differently from an `llm-pi-ai` route would make the same profile mean two
// things. Importing the one implementation keeps them one definition.
import { resolveProfiles } from '@deepseek-ai/dsh-llm-pi-ai/src/config.ts'
import { Config, plainOptions, resolveOptions, routeProfiles } from './config.ts'
import { documentSignature, readModelsDocument } from './document.ts'

export { Config, plainOptions, resolveOptions, fallbackProfile, routeProfiles, unconfiguredProfile, DEFAULT_API_KEY_ENV, DEFAULT_MODELS_FILE, DEFAULT_ROUTE, UNCONFIGURED_ROUTE, UNCONFIGURED_DISPLAY_NAME, UNCONFIGURED_BASE_URL } from './config.ts'
export type { DefaultRouteInput, Options, ResolvedOptions } from './config.ts'
export { ModelsDocumentError, parseModelsDocument, readModelsDocument, documentSignature } from './document.ts'
export type { ModelsDocument } from './document.ts'

/** Plugin name as the loader reports it. */
export const name = 'llm-harness-provider'
/** The language-model seam this plugin contributes routes to. */
export const inject = ['llm']

/**
 * How often the route source re-checks the document. The check is one `stat`,
 * and it is what lets a model added in the application appear in a session's
 * picker without a restart; file-system notifications are not used because the
 * document's directory may not exist until the application creates it.
 */
const POLL_INTERVAL_MS = 2_000

/**
 * Register the workspace's model routes.
 * @param ctx - plugin context carrying the `llm` seam.
 * @param config - live plugin configuration.
 */
export function apply(ctx: Context, config: Config): void {
  const options = resolveOptions(plainOptions(config))
  const file = isAbsolute(options.modelsFile) ? options.modelsFile : resolve(process.cwd(), options.modelsFile)

  let seen: string | undefined
  let providers: Record<string, PiAiProviderProfile> | undefined
  let resolvedSource: Record<string, PiAiProviderProfile> | undefined
  let resolved: ReadonlyMap<string, ResolvedPiAiProviderProfile> | undefined

  /** The current route profiles: the fallback route, overridden by the document per key. */
  const currentProviders = (): Record<string, PiAiProviderProfile> => {
    const signature = documentSignature(file)
    if (providers !== undefined && signature === seen) return providers
    const document = readModelsDocument(file)
    providers = routeProfiles(options.defaultRoute, document.providers)
    seen = signature
    return providers
  }

  /** Resolved profiles memoized by source identity, which keeps the adapter's snapshot stable. */
  const currentProfiles = (): ReadonlyMap<string, ResolvedPiAiProviderProfile> => {
    const source = currentProviders()
    if (resolved === undefined || resolvedSource !== source) {
      let next: ReadonlyMap<string, ResolvedPiAiProviderProfile>
      try {
        next = resolveProfiles(source, 'deferred')
      } catch (error: unknown) {
        throw new LlmError(
          `llm-harness-provider: the model routes in ${file} are not usable: ${error instanceof Error ? error.message : String(error)}`,
          'INVALID_CONFIG',
          { cause: error },
        )
      }
      resolved = next
      resolvedSource = source
    }
    return resolved
  }

  const resolveApiKey = async (
    provider: string,
    profile: ResolvedPiAiProviderProfile,
  ): Promise<string> => {
    const ref: CredentialRef = profile.apiKeyEnv ?? options.apiKeyEnv
    const credentials = ctx.get('credentials')
    const hit = credentials !== undefined
      ? (await credentials.resolve(ref))?.value
      // Without the seam the launch environment is the whole credential plane.
      : launchEnvironmentOf(ctx).get(ref)?.value
    if (hit !== undefined && hit.length > 0) return assertUsableApiKey(hit, 'llm-harness-provider', ref)
    throw new LlmError(
      `llm-harness-provider: no credential for route "${provider}"; store ${ref} through the credentials`
      + ` service, or put ${ref}=... in the workspace .env (a plugin that stores pi-ai-native credentials is`
      + ' not installed, so only the workspace credential plane answers)',
      'MISSING_CREDENTIAL',
    )
  }

  // This plugin never signs anyone in and never stores a pi-ai-native
  // credential: routes authenticate through the reference above. Both stores
  // therefore answer "nothing stored" honestly, and a write through pi-ai's own
  // flows fails loudly instead of pretending to have persisted a secret.
  const auth = {
    credentials: {
      read: async (): Promise<undefined> => undefined,
      list: async (): Promise<readonly []> => [],
      modify: async (): Promise<undefined> => {
        throw new LlmError(
          'llm-harness-provider: this plugin stores no pi-ai credentials; declare routes with apiKeyEnv and store the key in the workspace .env or the credentials service',
          'UNSUPPORTED_OPTION',
        )
      },
      delete: async (): Promise<void> => {
        throw new LlmError(
          'llm-harness-provider: this plugin stores no pi-ai credentials; there is nothing to delete',
          'UNSUPPORTED_OPTION',
        )
      },
    },
    authContext: {
      env: async (envName: string): Promise<string | undefined> => {
        const credentials = ctx.get('credentials')
        const hit = await credentials?.resolve(credentialRef(envName))
        return hit?.value ?? launchEnvironmentOf(ctx).get(envName)?.value
      },
      fileExists: async (): Promise<boolean> => false,
    },
  }

  const adapter = new PiAiAdapter({
    profiles: currentProfiles,
    resolveApiKey,
    auth,
    onReplayDegrade: ({ provider, model, reason }) => {
      ctx.logger.warn(
        `llm-harness-provider: unusable replay state on assistant history for route "${provider}/${model}";`
        + ` sending that message as provider-neutral content (${reason})`,
      )
    },
  })

  let registration: AdapterRegistrationHandle | undefined
  let registeredFacts: string | undefined

  /** Register, or replace, exactly the routes the document currently declares. */
  const sync = (): void => {
    const profiles = currentProfiles()
    const routes = [...profiles.keys()].sort()
    const facts = JSON.stringify(routes.map(route => {
      const profile = profiles.get(route)
      return [route, profile?.displayName, profile?.retryPolicy]
    }))
    if (facts === registeredFacts) return
    if (registration === undefined) {
      registration = ctx.llm.registerAdapter(routes, adapter)
    } else {
      registration.replace(routes)
    }
    registeredFacts = facts
    ctx.logger.info(`llm-harness-provider: serving ${routes.length} route(s): ${routes.join(', ')}`)
  }

  sync()
  const timer = setInterval(() => {
    try {
      sync()
    } catch (error: unknown) {
      // A document edited into an invalid state keeps the last good routes
      // serving; the failure is reported once per poll instead of unwinding.
      ctx.logger.error(String(error))
    }
  }, POLL_INTERVAL_MS)
  timer.unref()
  ctx.effect(() => () => { clearInterval(timer) })
}
