/**
 * The models document is the application's half of the route contract: the
 * application writes it, this plugin reads it, and every field a route may
 * carry is declared here rather than copied from the application. These tests
 * pin the two ends of that contract — what a document may say, and what the
 * plugin's own configuration resolves to when a deployment says nothing.
 */

import { strict as assert } from 'node:assert'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { Config, DEFAULT_API_KEY_ENV, DEFAULT_MAX_TOKENS, DEFAULT_MODELS_FILE, UNCONFIGURED_ROUTE, fallbackProfile, plainOptions, resolveDefaultMaxTokens, resolveOptions, routeProfiles, unconfiguredProfile } from '../src/index.ts'
import { ModelsDocumentError, documentSignature, parseModelsDocument, readModelsDocument } from '../src/index.ts'

/** A document with one complete route, as the application writes it. */
function documentText(overrides: Record<string, unknown> = {}) {
  return JSON.stringify({
    active: { provider: 'default', model: 'deepseek-chat' },
    providers: {
      default: {
        displayName: 'DeepSeek',
        api: 'openai-completions',
        baseURL: 'https://api.deepseek.com/v1',
        apiKeyEnv: 'AI_API_KEY',
        models: [{ id: 'deepseek-chat', name: 'deepseek-chat', contextWindow: 131072, maxTokens: 16384 }],
      },
    },
    ...overrides,
  })
}

function write(directory: string, text: string): string {
  const path = join(directory, 'models.json')
  writeFileSync(path, text)
  return path
}

test('a document with one route resolves to that route', () => {
  const document = parseModelsDocument(JSON.parse(documentText()), 'models.json')
  assert.deepEqual(Object.keys(document.providers), ['default'])
  assert.equal(document.providers.default?.baseURL, 'https://api.deepseek.com/v1')
  // `active` is the application's own selection and is accepted without being
  // read: the plugin serves routes, it does not choose one.
  assert.deepEqual(Object.keys(document), ['providers'])
})

test('a workspace with no document yet is an empty route set, not a failure', () => {
  const directory = mkdtempSync(join(tmpdir(), 'harness-models-'))
  const path = join(directory, 'models.json')
  assert.deepEqual(readModelsDocument(path).providers, {})
  assert.equal(documentSignature(path), undefined)
})

test('a model must name an id, and an explicit empty model list is refused', () => {
  const emptyModels = { providers: { broken: { baseURL: 'https://example.test/v1', models: [] } } }
  assert.throws(() => parseModelsDocument(emptyModels, 'models.json'), /omit it to serve the installed catalog/u)
  const unnamed = { providers: { broken: { baseURL: 'https://example.test/v1', models: [{}] } } }
  // The failure names the exact entry, so "which model" is answered by the message.
  assert.throws(() => parseModelsDocument(unnamed, 'models.json'), /models\[0\]\.id must be a non-empty string/u)
  const notAList = { providers: { broken: { baseURL: 'https://example.test/v1', models: {} } } }
  assert.throws(() => parseModelsDocument(notAList, 'models.json'), /must be an array/u)
})

test('omitting models is the catalog posture, not a typo', () => {
  const document = parseModelsDocument({ providers: { openai: { apiKeyEnv: 'AI_API_KEY' } } }, 'models.json')
  assert.equal(document.providers.openai?.models, undefined)
})

test('a misspelled field is refused instead of silently ignored', () => {
  const wrong = { providers: { default: { baseURL: 'https://example.test/v1', models: [{ id: 'a' }], apiKeyEnvv: 'AI_API_KEY' } } }
  assert.throws(() => parseModelsDocument(wrong, 'models.json'), /apiKeyEnvv/u)
  const wrongTop = { provider: { default: {} } }
  assert.throws(() => parseModelsDocument(wrongTop, 'models.json'), /provider/u)
})

test('an unreadable document is a named failure, not an empty route set', () => {
  const directory = mkdtempSync(join(tmpdir(), 'harness-models-'))
  const path = write(directory, '{ this is not json')
  assert.throws(() => readModelsDocument(path), ModelsDocumentError)
})

test('editing the document changes its signature, which is what re-reads it', () => {
  const directory = mkdtempSync(join(tmpdir(), 'harness-models-'))
  const path = write(directory, documentText())
  const first = documentSignature(path)
  assert.notEqual(first, undefined)
  writeFileSync(path, documentText({ active: { provider: 'default', model: 'deepseek-reasoner' } }))
  assert.notEqual(documentSignature(path), first)
})

test('configuration defaults to the workspace document and AI_API_KEY', () => {
  const options = resolveOptions(plainOptions(Config({}) as never))
  assert.equal(options.modelsFile, DEFAULT_MODELS_FILE)
  assert.equal(options.apiKeyEnv, DEFAULT_API_KEY_ENV)
  assert.equal(options.defaultRoute, undefined)
})

test('an absent fallback route is optional, a declared one requires its endpoint and model', () => {
  // Regression: schemastery gives a bare object schema the default `{}`, so a
  // nested required field would otherwise make the whole route mandatory.
  assert.equal((Config({}) as { defaultRoute?: unknown }).defaultRoute, undefined)
  const declared = Config({ defaultRoute: { baseURL: 'https://api.deepseek.com/v1', model: 'deepseek-chat' } }) as {
    defaultRoute?: { api?: string }
  }
  assert.equal(declared.defaultRoute?.api, 'openai-completions')
  assert.throws(() => Config({ defaultRoute: { model: 'deepseek-chat' } }), /baseURL/u)
})

test('the fallback route becomes one pi-ai profile under the configured route key', () => {
  const profile = fallbackProfile('default', {
    displayName: 'DeepSeek',
    baseURL: 'https://api.deepseek.com/v1',
    model: 'deepseek-chat',
    contextWindow: 131072,
  })
  assert.equal(profile.displayName, 'DeepSeek')
  assert.equal(profile.api, 'openai-completions')
  assert.deepEqual(profile.models?.map(model => model.id), ['deepseek-chat'])
  assert.equal(profile.models?.[0]?.contextWindow, 131072)
})

test('a workspace with nothing declared still serves the placeholder route', () => {
  // Regression: the acp row names a route when it creates a session, so an
  // empty route set is not "nothing to serve" — it is an unopenable session.
  const profiles = routeProfiles(undefined, {})
  assert.deepEqual(Object.keys(profiles), [UNCONFIGURED_ROUTE])
  assert.deepEqual(profiles[UNCONFIGURED_ROUTE]?.models?.map(model => model.id), [UNCONFIGURED_ROUTE])
  // The placeholder must be obviously unusable, not a silent default that
  // answers with someone else's service.
  assert.match(profiles[UNCONFIGURED_ROUTE]?.baseURL ?? '', /^http:\/\/127\.0\.0\.1:/u)
  assert.equal(profiles[UNCONFIGURED_ROUTE]?.displayName, unconfiguredProfile().displayName)
})

test('any declared route suppresses the placeholder', () => {
  const fallback = { route: 'default', profile: fallbackProfile('default', { baseURL: 'https://api.deepseek.com/v1', model: 'deepseek-chat' }) }
  assert.deepEqual(Object.keys(routeProfiles(fallback, {})), ['default'])
  const document = { default: fallbackProfile('default', { baseURL: 'https://example.test/v1', model: 'a' }) }
  const merged = routeProfiles(fallback, document)
  assert.deepEqual(Object.keys(merged), ['default'])
  // The document overrides the fallback per key; the placeholder stays out.
  assert.equal(merged.default?.baseURL, 'https://example.test/v1')
})


test('invalid endpoints fail before a request without echoing embedded secrets', () => {
  for (const baseURL of ['openrouter.ai/api/v1', 'https://', 'file:///tmp/api', 'https://host/v1 secret', 'https://secret@host/v1', 'https://host/v1?key=secret']) {
    assert.throws(() => parseModelsDocument({ providers: { test: { baseURL } } }, 'models.json'), error => {
      assert.match(String(error), /providers.test: baseURL must/u)
      assert.doesNotMatch(String(error), /secret/u)
      return true
    })
    assert.throws(() => fallbackProfile('default', { baseURL, model: 'test' }), /baseURL must/u)
  }
  for (const providers of [null, [], 'wrong']) {
    assert.throws(() => parseModelsDocument({ providers }, 'models.json'), /providers must be an object/u)
  }
})

test("a route with no output cap of its own gets this harness's conservative default on its MODEL entry, whoever declared it", () => {
  // The exact bug this pins, twice over. First: a route added through the
  // app's model dialog — not the cordis.yml fallback — was left with no cap
  // at all, so dsh's own larger fallback applied and a provider with a
  // smaller per-model ceiling (Groq is one) rejected every request outright.
  // Second, a real regression in the first fix for it: the harness patched
  // the profile's own `defaultMaxTokens` field, which only sizes dsh's
  // internal catalog bookkeeping and never reaches the wire request at all —
  // only a PER-MODEL `maxTokens` becomes the adapter's `configuredMaxTokens`
  // and so the agent loop's actual request default. Setting the wrong field
  // looked identical in a config dump and still produced the exact same
  // failure in practice.
  delete process.env.AI_MAX_TOKENS
  const document = { qwen: fallbackProfile('qwen', { baseURL: 'https://api.groq.com/openai/v1', model: 'qwen/qwen3.8-27b' }) }
  const profiles = routeProfiles(undefined, document)
  assert.equal(profiles.qwen?.models?.[0]?.maxTokens, DEFAULT_MAX_TOKENS)
  assert.equal(resolveDefaultMaxTokens(), DEFAULT_MAX_TOKENS)
})

test('a model that already declares its own output cap is left exactly as configured', () => {
  const declared = fallbackProfile('qwen', { baseURL: 'https://api.groq.com/openai/v1', model: 'qwen/qwen3.8-27b', maxTokens: 16_384 })
  const document = { qwen: declared }
  const profiles = routeProfiles(undefined, document)
  assert.equal(profiles.qwen?.models?.[0]?.maxTokens, 16_384, 'an explicit choice is never overridden')
})

test('a route serving the installed catalog as-is (no models declared) is left alone', () => {
  const document = { qwen: { baseURL: 'https://api.groq.com/openai/v1' } }
  const profiles = routeProfiles(undefined, document)
  assert.equal(profiles.qwen?.models, undefined, "the installed catalog's own capacities are real capabilities, not a gap")
})

test('AI_MAX_TOKENS raises or lowers the default for every model that leaves it unset', () => {
  process.env.AI_MAX_TOKENS = '4096'
  try {
    assert.equal(resolveDefaultMaxTokens(), 4_096)
    const document = { a: fallbackProfile('a', { baseURL: 'https://example.test/v1', model: 'x' }) }
    assert.equal(routeProfiles(undefined, document).a?.models?.[0]?.maxTokens, 4_096)
  } finally {
    delete process.env.AI_MAX_TOKENS
  }
  process.env.AI_MAX_TOKENS = 'not-a-number'
  try {
    assert.equal(resolveDefaultMaxTokens(), DEFAULT_MAX_TOKENS, 'garbage input falls back rather than producing NaN')
  } finally {
    delete process.env.AI_MAX_TOKENS
  }
})
