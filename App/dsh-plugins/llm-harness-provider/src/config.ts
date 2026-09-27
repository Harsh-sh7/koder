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

import { validateBaseURL } from './document.ts'

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
/**
 * Output tokens one response may use, for any route that declares neither its
 * own `defaultMaxTokens` nor a per-model `maxTokens`.
 *
 * dsh's own fallback, applied at exactly that point, is 32768 — larger than
 * some providers' real per-model ceiling even though it is well under the
 * model's context window (Groq is one: it rejects the request outright rather
 * than clamping it). Free-tier ceilings run tighter than "larger than a
 * per-model cap" too, though: measured against a real Groq free key, its
 * account-wide limit was 1,000 *output* tokens a minute, shared across every
 * request in flight — a number no single deployment-wide constant can dodge
 * for every provider at once, but this is picked to fit comfortably under the
 * tightest free tiers seen in practice while still leaving room for an actual
 * file's worth of output, rather than only clearing the bar of "some cap".
 * This is what makes a route someone adds through the model dialog — on
 * OpenRouter, Groq, or anywhere else — safe by default without hand-tuning
 * every route the moment it turns out to be a free one.
 */
export const DEFAULT_MAX_TOKENS = 2_000

/**
 * Working context one route may use, for any per-model entry that declares no
 * `contextWindow` of its own.
 *
 * Deliberately not read from `AI_CONTEXT_WINDOW`: that variable's own default
 * (131072, applied to the `cordis.yml` fallback route) is sized for a capable
 * route and would hand the same large number to every route added through the
 * model dialog if reused here — including a free-tier key whose real
 * per-minute budget is a few thousand tokens *total*, input and output
 * combined. An agent loop resends its whole working context on every step, so
 * an unbounded window is exactly what turns "wrote a bit too much" into
 * "blew the account's rate limit on request one." Compaction keeps the
 * conversation inside this window, so it bounds what every step resends.
 *
 * It must still sit well above the harness's own fixed prefix: the system
 * prompt plus every tool schema is ~5k tokens before the conversation says a
 * word (measured from a recorded request), and the engine sizes each
 * response's output budget as window minus input. A 5k window once shipped
 * here, and every request went out with `max_tokens: 1` — the model thought
 * one token ("The") and the turn stopped dead.
 */
export const DEFAULT_CONTEXT_WINDOW = 32_768

/**
 * The smallest context window a route is ever given, whatever it declares.
 *
 * Below this, the fixed prefix (system prompt + tool schemas, ~5k tokens)
 * plus a conversation plus a response cannot fit, and the engine's output
 * budget (window minus input) collapses to a handful of tokens — every turn
 * then ends after the model's first word with no error, which is far worse
 * than any rate limit. So this is a correctness floor, not a preference, and
 * it applies to an explicitly configured value too.
 */
export const MIN_CONTEXT_WINDOW = 16_384

/**
 * The configured output-token cap, from `AI_MAX_TOKENS` — read directly from
 * the process environment because it is a plain tuning number, not a
 * credential, and this plugin already runs inside the engine process
 * `run-acp.mjs` set it in.
 * @returns the configured value, or {@link DEFAULT_MAX_TOKENS} when unset or unparsable.
 */
export function resolveDefaultMaxTokens(): number {
  const raw = process.env.AI_MAX_TOKENS?.trim()
  if (raw === undefined || raw.length === 0) return DEFAULT_MAX_TOKENS
  const parsed = Number(raw)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_MAX_TOKENS
}

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
  validateBaseURL(input.baseURL)
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
  defaultMaxTokens: number = resolveDefaultMaxTokens(),
  defaultContextWindow: number = DEFAULT_CONTEXT_WINDOW,
): Record<string, PiAiProviderProfile> {
  const declared = {
    ...fallback === undefined ? {} : { [fallback.route]: fallback.profile },
    ...document,
  }
  if (Object.keys(declared).length === 0) return { [UNCONFIGURED_ROUTE]: unconfiguredProfile() }
  // A model that names no output cap, or no working context, of its own gets
  // this harness's conservative ones, so a route added through the model
  // dialog — not only the cordis.yml fallback — is safe by default on a
  // provider like Groq or a free OpenRouter pool, on both halves of what a
  // request actually costs (what it sends, and what it may get back).
  //
  // maxTokens has to be a per-MODEL field, not the profile's own
  // `defaultMaxTokens`: only a per-model cap becomes the adapter's
  // `configuredMaxTokens` and so the agent loop's actual request default
  // (`llm-pi-ai/adapter.ts`'s `modelInfo` — "Only a cap the deployment
  // configured is a request default; the catalog's `maxTokens` sizes the
  // model and stops there"). A route-level `defaultMaxTokens` only sizes the
  // catalog's own bookkeeping and never reaches the wire request at all,
  // which is exactly the shape a real bug here once took: the field was set,
  // correctly, and still had no effect. `models` omitted entirely (the route
  // serves dsh's installed catalog as-is) is left alone — that catalog's own
  // capacities are real capabilities, not a gap to fill.
  return Object.fromEntries(
    Object.entries(declared).map(([route, profile]) => [
      route,
      profile.models === undefined
        ? profile
        : {
          ...profile,
          models: profile.models.map(model => ({
            ...model,
            ...model.maxTokens === undefined ? { maxTokens: defaultMaxTokens } : {},
            contextWindow: Math.max(model.contextWindow ?? defaultContextWindow, MIN_CONTEXT_WINDOW),
          })),
        },
    ]),
  )
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
