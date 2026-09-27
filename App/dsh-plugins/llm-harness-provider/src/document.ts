/**
 * The workspace models document: the application-owned route registry the
 * harness reads on every request.
 *
 * The document is a superset of the pi-ai provider dict, because the harness
 * reads it as provider profiles directly: `providers` holds the profiles, and
 * `active` records which route the application selected, which is the
 * application's own concern and is ignored here. Keeping the file in the
 * profile vocabulary means there is no second schema to drift: `resolveProfiles`
 * validates the same fields a `cordis.yml` row would supply.
 *
 * @module @harness/dsh-llm-harness-provider/document
 */

import { readFileSync, statSync } from 'node:fs'
import type { PiAiProviderProfile } from '@deepseek-ai/dsh-llm-pi-ai'

/** Parse and validation failure naming the exact document location. */
export class ModelsDocumentError extends Error {
  /**
   * @param path - the document path as opened.
   * @param detail - what is wrong, as one sentence.
   */
  constructor(path: string, detail: string) {
    super(`models document ${path}: ${detail}`)
    this.name = 'ModelsDocumentError'
  }
}

/** One workspace models document. */
export interface ModelsDocument {
  /** Route profiles keyed by provider route name. */
  readonly providers: Record<string, PiAiProviderProfile>
}

/** Provider-level fields `resolveProfiles` reads; anything else is a typo. */
const ROUTE_FIELDS = new Set([
  'apiKeyEnv', 'displayName', 'api', 'baseURL', 'models', 'modelOverrides', 'compat',
  'defaultContextWindow', 'defaultMaxTokens', 'defaultInput', 'headers', 'reasoning',
  'thinkingBudgets', 'cacheRetention', 'transport', 'timeoutMs', 'websocketConnectTimeoutMs',
  'streamIdleTimeoutMs', 'maxRequestImageBytes', 'requestImagePixelBudget', 'requestImageMaxBytes',
  'retryPolicy',
])

/** Fields of one `models` entry; a misspelled one would silently read as defaults. */
const MODEL_FIELDS = new Set([
  'id', 'name', 'contextWindow', 'maxTokens', 'input', 'reasoningEfforts', 'compat',
])

/** One plain JSON object. */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** Refuse a value the document cannot mean, naming the field path. */
function assertNoUnknownKeys(path: string, value: Record<string, unknown>, allowed: ReadonlySet<string>): void {
  const unknown = Object.keys(value).filter(key => !allowed.has(key))
  if (unknown.length === 0) return
  const allowedList = [...allowed].sort().join(', ')
  throw new ModelsDocumentError(path, `unknown field${unknown.length > 1 ? 's' : ''} ${unknown.join(', ')}; known fields are ${allowedList}`)
}

/** Validate endpoint syntax without including potentially secret input in errors. */
export function validateBaseURL(value: unknown): void {
  const invalid = 'baseURL must be an absolute http:// or https:// URL with a host'
  if (typeof value !== 'string' || /\s/u.test(value) || !/^https?:\/\//u.test(value)) throw new Error(invalid)
  let url: URL
  try { url = new URL(value) } catch { throw new Error(invalid) }
  if (!url.hostname) throw new Error(invalid)
  if (url.username || url.password || url.search || url.hash) {
    throw new Error('baseURL must not contain credentials, a query, or a fragment; use apiKeyEnv for the key')
  }
}

/**
 * Validate one route profile without reimplementing profile semantics:
 * structure and field names are checked here so a typo names its line, and
 * `resolveProfiles` remains the authority on what the fields mean.
 *
 * `models` may be omitted, which is the catalog posture: the route serves
 * whatever the installed pi-ai catalog ships under that key. That is what makes
 * a documented OpenAI or Anthropic route one line, and a key the catalog does
 * not know reports itself unserviceable through the resolution diagnostics
 * rather than being refused here.
 * @param path - document location, for the failure message.
 * @param route - route key as it appears in the document.
 * @param value - parsed route value.
 */
function assertRoute(path: string, route: string, value: unknown): void {
  const where = `providers.${route}`
  if (!isRecord(value)) throw new ModelsDocumentError(path, `${where} must be an object`)
  assertNoUnknownKeys(path, value, ROUTE_FIELDS)
  if (value.baseURL !== undefined) {
    try { validateBaseURL(value.baseURL) } catch (error) {
      throw new ModelsDocumentError(path, `${where}: ${(error as Error).message}`)
    }
  }
  if (value.models !== undefined && !Array.isArray(value.models)) {
    throw new ModelsDocumentError(path, `${where}.models must be an array`)
  }
  // An explicit empty list is refused because it cannot mean the catalog
  // posture above: the writer had to type an empty array to get here, and the
  // route it describes can never answer a request.
  if (Array.isArray(value.models) && value.models.length === 0) {
    throw new ModelsDocumentError(path, `${where}.models is empty; omit it to serve the installed catalog instead`)
  }
  for (const [index, model] of (value.models ?? []).entries()) {
    const modelWhere = `${where}.models[${index}]`
    if (!isRecord(model)) throw new ModelsDocumentError(path, `${modelWhere} must be an object`)
    assertNoUnknownKeys(path, model, MODEL_FIELDS)
    if (typeof model.id !== 'string' || model.id.length === 0) {
      throw new ModelsDocumentError(path, `${modelWhere}.id must be a non-empty string`)
    }
  }
}

/**
 * Read one parsed document.
 * @param raw - the document as parsed JSON.
 * @param path - document location, for failure messages.
 * @returns the route profiles, detached from `raw`.
 * @throws {ModelsDocumentError} for a document that is not an object with route profiles.
 */
export function parseModelsDocument(raw: unknown, path: string): ModelsDocument {
  if (!isRecord(raw)) throw new ModelsDocumentError(path, 'the document must be a JSON object')
  const unknown = Object.keys(raw).filter(key => key !== 'providers' && key !== 'active')
  if (unknown.length > 0) throw new ModelsDocumentError(path, `unknown top-level field ${unknown.join(', ')}; expected providers and active`)
  if (raw.providers !== undefined && !isRecord(raw.providers)) {
    throw new ModelsDocumentError(path, 'providers must be an object')
  }
  const providers: Record<string, PiAiProviderProfile> = {}
  for (const [route, value] of Object.entries(raw.providers ?? {})) {
    if (route.length === 0) throw new ModelsDocumentError(path, 'provider route names must be non-empty')
    assertRoute(path, route, value)
    providers[route] = structuredClone(value) as PiAiProviderProfile
  }
  return { providers }
}

/**
 * Read the workspace models document.
 *
 * A missing file is an empty registry rather than a failure: the harness boots
 * before a person has added any model, and the onboarding surface is the
 * application's answer to that state. A file that exists but does not parse is
 * a configuration failure and throws, because silently ignoring it would run
 * the session on a different route than the document says.
 * @param path - absolute document path.
 * @returns the parsed document, or an empty registry when the file is absent.
 * @throws {ModelsDocumentError} for unreadable or invalid content.
 */
export function readModelsDocument(path: string): ModelsDocument {
  let text: string
  try {
    text = readFileSync(path, 'utf8')
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { providers: {} }
    throw new ModelsDocumentError(path, `cannot be read: ${String(error)}`)
  }
  let raw: unknown
  try {
    raw = JSON.parse(text) as unknown
  } catch (error: unknown) {
    throw new ModelsDocumentError(path, `is not valid JSON: ${String(error)}`)
  }
  return parseModelsDocument(raw, path)
}

/**
 * Identity of the document's current bytes, cheap enough to take per request.
 * @param path - absolute document path.
 * @returns a signature that changes whenever the file is rewritten, or undefined when absent.
 */
export function documentSignature(path: string): string | undefined {
  try {
    const stats = statSync(path)
    return `${String(stats.mtimeMs)}:${String(stats.size)}`
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined
    throw new ModelsDocumentError(path, `cannot be inspected: ${String(error)}`)
  }
}
