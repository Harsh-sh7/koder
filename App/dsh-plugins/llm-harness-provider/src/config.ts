/**
 * Deployment configuration of the harness model-provider plugin.
 *
 * A deployment declares either a workspace models document, a fallback route
 * for a machine nobody has configured yet, or both. The fallback exists so a
 * clean clone boots with nothing but `AI_API_KEY`: it is a `cordis.yml` route
 * whose fields an environment override can replace without touching source.
 *
 * @module @harness/dsh-llm-harness-provider/config
 */

import type { Volatile } from '@deepseek-ai/cordis'
import z from '@deepseek-ai/schemastery'
import { credentialRef, type CredentialRef } from '@deepseek-ai/dsh-credentials'
import type { PiAiProviderProfile } from '@deepseek-ai/dsh-llm-pi-ai'

/** Workspace-relative location of the document the application writes. */
export const DEFAULT_MODELS_FILE = '.harness/models.json'
/** Credential reference every route falls back to; the workspace `.env` supplies it. */
export const DEFAULT_API_KEY_ENV = 'AI_API_KEY'
/** Route key of the `cordis.yml` fallback route. */
export const DEFAULT_ROUTE = 'default'
/** Route key of the placeholder served when no route is declared anywhere. */
export const UNCONFIGURED_ROUTE = 'unconfigured'
/** Display name of the placeholder route. */
export const UNCONFIGURED_DISPLAY_NAME = 'No model configured'
/**
 * Endpoint of the placeholder route: the discard service, so a prompt fails at
 * once — naming the route — instead of hanging on a connection.
 */
export const UNCONFIGURED_BASE_URL = 'http://127.0.0.1:9/v1'

/** One fallback route as `cordis.yml` declares it. */
export interface DefaultRouteInput {
  /** Label shown by model pickers. */
  displayName?: string
  /** Wire protocol, as a pi-ai api id. */
  api?: string
  /** OpenAI-compatible endpoint root. */
  baseURL: string
  /** Model id sent to that endpoint. */
  model: string
  /** Context capacity to advertise; defaults to the pi-ai default. */
  contextWindow?: number
  /** Per-response output cap to advertise. */
  maxTokens?: number
}

/** Plugin configuration with the credential reference kept live. */
export interface Config {
  /** Workspace-relative (or absolute) models document path. */
  modelsFile: string
  /** Credential reference resolved per request. */
  apiKeyEnv: Volatile<string>
  /** Fallback route for a workspace that has not added a model yet. */
  defaultRoute?: DefaultRouteInput
}

/**
 * The fallback route as a runtime schema, mirroring {@link DefaultRouteInput}.
 *
 * The `undefined` alternative is load-bearing: schemastery gives a bare object
 * schema the default `{}`, so an absent `defaultRoute` would otherwise be
 * *materialized* as an empty object and then rejected for the required fields it
 * does not have. This is the same spelling the repository uses for a
 * configuration value that is absent rather than empty.
 */
const defaultRouteSchema = z.union([
  z.object({
    displayName: z.string(),
    api: z.string().default('openai-completions'),
    baseURL: z.string().required(),
    model: z.string().required(),
    contextWindow: z.number().step(1).min(1),
    maxTokens: z.number().step(1).min(1),
  }),
  z.const(undefined),
])

/** Runtime schema for {@link Config}. */
export const Config = z.object({
  modelsFile: z.string().default(DEFAULT_MODELS_FILE),
  apiKeyEnv: z.string().role('credential-ref').default(DEFAULT_API_KEY_ENV).volatile(),
  defaultRoute: defaultRouteSchema,
})

/** Plain deployment inputs, detached from the live configuration. */
export interface Options {
  /** Models document path; defaults to {@link DEFAULT_MODELS_FILE}. */
  modelsFile?: string
  /** Credential reference; defaults to {@link DEFAULT_API_KEY_ENV}. */
  apiKeyEnv?: string
  /** Fallback route declaration. */
  defaultRoute?: DefaultRouteInput
}

/** Settings captured for one request. */
export interface ResolvedOptions {
  /** Workspace-relative or absolute models document path. */
  modelsFile: string
  /** Credential reference used by routes that name none themselves. */
  apiKeyEnv: CredentialRef
  /** Fallback route profile under {@link DEFAULT_ROUTE}, when declared. */
  defaultRoute?: { route: string; profile: PiAiProviderProfile }
}

/**
 * Build the route profile one fallback declaration stands for.
 * @param route - route key to register the profile under.
 * @param input - the declaration as configured.
 * @returns the profile, with one model entry named after the model id.
 */
export function fallbackProfile(route: string, input: DefaultRouteInput): PiAiProviderProfile {
  return {
    displayName: input.displayName ?? route,
    api: input.api ?? 'openai-completions',
    baseURL: input.baseURL,
    models: [{
      id: input.model,
      name: input.model,
      ...input.contextWindow === undefined ? {} : { contextWindow: input.contextWindow },
      ...input.maxTokens === undefined ? {} : { maxTokens: input.maxTokens },
    }],
  }
}

/**
 * The placeholder profile served when no route is declared anywhere.
 *
 * A session is created against a route by name, so "nothing configured" cannot
 * mean "no adapter registered" — it means one route that cannot complete a
 * request. That is this profile: it keeps `session/new` working on a workspace
 * nobody has configured yet, and every prompt on it fails at once with the
 * route's name in the error, which is what the application's empty state asks
 * the user to fix.
 * @returns the placeholder profile.
 */
export function unconfiguredProfile(): PiAiProviderProfile {
  return {
    displayName: UNCONFIGURED_DISPLAY_NAME,
    api: 'openai-completions',
    baseURL: UNCONFIGURED_BASE_URL,
    models: [{ id: UNCONFIGURED_ROUTE, name: UNCONFIGURED_DISPLAY_NAME }],
  }
}

/**
 * The profiles in force for one reading of the deployment.
 *
 * The `cordis.yml` fallback is the base layer — it exists so a clean clone with
 * `AI_API_KEY` boots on a declared route — and the workspace document overrides
 * it per key. When neither declares anything, the placeholder stands in, so the
 * route set is never empty and a session can always name a provider.
 * @param fallback - the resolved `defaultRoute`, when the deployment declared one.
 * @param document - the workspace document's routes.
 * @returns the route profiles to serve.
 */
export function routeProfiles(
  fallback: { route: string; profile: PiAiProviderProfile } | undefined,
  document: Record<string, PiAiProviderProfile>,
): Record<string, PiAiProviderProfile> {
  const declared = {
    ...fallback === undefined ? {} : { [fallback.route]: fallback.profile },
    ...document,
  }
  return Object.keys(declared).length === 0 ? { [UNCONFIGURED_ROUTE]: unconfiguredProfile() } : declared
}

/**
 * Detach the live configuration from its volatile fields.
 * @param config - live plugin configuration.
 * @returns plain deployment inputs.
 */
export function plainOptions(config: Config): Options {
  return {
    modelsFile: config.modelsFile,
    apiKeyEnv: config.apiKeyEnv.get(),
    ...config.defaultRoute === undefined ? {} : { defaultRoute: config.defaultRoute },
  }
}

/**
 * Resolve plain inputs into the settings one request reads.
 * @param options - plain deployment inputs.
 * @returns validated settings with the credential reference and route profile built.
 */
export function resolveOptions(options: Options): ResolvedOptions {
  return {
    modelsFile: options.modelsFile ?? DEFAULT_MODELS_FILE,
    apiKeyEnv: credentialRef(options.apiKeyEnv ?? DEFAULT_API_KEY_ENV),
    ...options.defaultRoute === undefined
      ? {}
      : { defaultRoute: { route: DEFAULT_ROUTE, profile: fallbackProfile(DEFAULT_ROUTE, options.defaultRoute) } },
  }
}
