/**
 * The frozen-spec vocabulary: digest identity, the durable-log fold that
 * recognizes a session's spec, the check that decides when a spec must be
 * restated, and the restatement itself.
 *
 * The spec is not a plugin-private record. It is the session's first
 * substantial user-authored message, read back out of the durable log, and that
 * is what lets a resumed or forked session keep it: an out-of-tree plugin cannot
 * mark its own session event `ignorable`, and a stored log carrying an unknown
 * unmarked event refuses to reload, so no plugin-owned event type exists here.
 *
 * @module @harness/dsh-spec-lock/spec
 */

import { createHash } from 'node:crypto'

import { boundContextSummary } from '@deepseek-ai/dsh-llm'
import type { ContentBlock, ContextFormed, Message } from '@deepseek-ai/dsh-llm'
import type { SessionEvent } from '@deepseek-ai/dsh-session'
import type { ProjectionDefinition } from '@deepseek-ai/dsh-session-projection'
import { z as zod } from 'zod'
import type { ZodType } from 'zod'

declare module '@deepseek-ai/dsh-llm' {
  interface MessageSourceMap {
    'spec-lock': SpecLockSource
  }
}

declare module '@deepseek-ai/dsh-session-projection/types' {
  interface SessionProjectionStateMap {
    specLock: SpecLockState
  }
  interface SessionProjectionMap {
    specLock: SpecLockView
  }
}

/** Message-source kind of every message this plugin writes. */
export const SPEC_LOCK_SOURCE = 'spec-lock'

/**
 * Source of a plugin-authored spec message: the digest of the version it
 * carries, plus the marker that distinguishes an approved amendment (this
 * message IS the new spec) from a restatement of the current one.
 */
export type SpecLockSource = {
  kind: typeof SPEC_LOCK_SOURCE
  /** Digest of the spec text this message carries. */
  specDigest: string
  /** Present only on an approved amendment. */
  specAmendment?: true
} & ContextFormed

/** Default for {@link Config.maxInlineChars}: longest spec restated in full. */
export const DEFAULT_MAX_INLINE_CHARS = 1500

/** Default for {@link Config.minChars}: every non-empty text qualifies. */
export const DEFAULT_MIN_CHARS = 1

/**
 * Default for {@link Config.refreezeMinChars}: a later user prompt at least this
 * long is a new task and replaces the spec. Shorter ones ("continue", "yes,
 * do that") are follow-ups on the current spec and leave it alone.
 */
export const DEFAULT_REFREEZE_MIN_CHARS = 80

/** Fold state of one session's frozen spec. Plain JSON: the projection cache persists it. */
export interface SpecLockState {
  /** The frozen spec text, or null while no message has qualified. */
  readonly text: string | null
  /** Digest of {@link SpecLockState.text}, or null while unlocked. */
  readonly digest: string | null
  /** Length of {@link SpecLockState.text}. */
  readonly chars: number
  /** Time of the user message the spec was frozen from, or null. */
  readonly lockedAt: number | null
  /** Time of the latest approved amendment, or null. */
  readonly amendedAt: number | null
  /** How many approved amendments the spec has taken. */
  readonly amendments: number
}

/** Client-visible summary of the frozen spec: everything but its text. */
export interface SpecLockView {
  /** Whether a spec is frozen for the session. */
  readonly locked: boolean
  /** Digest of the frozen text, or null while unlocked. */
  readonly digest: string | null
  /** Length of the frozen text. */
  readonly chars: number
  /** How many approved amendments the spec has taken. */
  readonly amendments: number
}

const specLockStateSchema: ZodType<SpecLockState> = zod.object({
  text: zod.string().nullable(),
  digest: zod.string().nullable(),
  chars: zod.number().int().min(0),
  lockedAt: zod.number().nullable(),
  amendedAt: zod.number().nullable(),
  amendments: zod.number().int().min(0),
}).strict()

const specLockViewSchema: ZodType<SpecLockView> = zod.object({
  locked: zod.boolean(),
  digest: zod.string().nullable(),
  chars: zod.number().int().min(0),
  amendments: zod.number().int().min(0),
}).strict()

/** The empty fold state: nothing frozen yet. */
export function emptySpecLockState(): SpecLockState {
  return { text: null, digest: null, chars: 0, lockedAt: null, amendedAt: null, amendments: 0 }
}

/**
 * Digest identity of one spec version, short enough to sit in prose.
 * @param text - the spec text.
 * @returns `sha256:` and the first 12 hex digits of the UTF-8 digest.
 */
export function digestOf(text: string): string {
  return `sha256:${createHash('sha256').update(text, 'utf8').digest('hex').slice(0, 12)}`
}

/**
 * The text one message contributes: its text blocks in order, trimmed. Images
 * and other non-text blocks are not spec text.
 * @param message - the message to read.
 * @returns the concatenated text, `''` when the message carries none.
 */
export function messageText(message: { readonly content: readonly ContentBlock[] }): string {
  const parts: string[] = []
  for (const block of message.content) {
    if (block.type === 'text') parts.push(block.text)
  }
  return parts.join('\n').trim()
}

/**
 * Fold one committed event into the spec state.
 *
 * The spec is the first user-authored message whose text meets `minChars`:
 * `user` is the one source kind the session's participants write, so
 * context-injected user messages — this plugin's restatements included — cannot
 * become the spec. An approved amendment, marked in its own source, replaces
 * the text; a restatement never does.
 *
 * With `refreezeMinChars` set, a later user-authored message at least that long
 * is a new task and re-freezes the spec. One session is routinely handed one
 * issue after another; freezing only the first would restate issue one into the
 * fortieth issue's context and hold every later issue to the first one's text.
 * @param state - the state covering all earlier events.
 * @param event - the next committed event.
 * @param minChars - shortest text a message may freeze with.
 * @param refreezeMinChars - shortest later message that replaces the spec; undefined never replaces it.
 * @returns the next state, the same reference when this event changes nothing.
 */
export function applySpecLockEvent(
  state: SpecLockState,
  event: SessionEvent,
  minChars: number,
  refreezeMinChars?: number,
): SpecLockState {
  if (event.type !== 'user/message') return state
  const source = event.data.source
  if (source.kind === SPEC_LOCK_SOURCE) {
    if (source.specAmendment !== true) return state
    const text = messageText(event.data)
    if (text.length === 0) return state
    return {
      ...state,
      text,
      digest: digestOf(text),
      chars: text.length,
      amendedAt: event.time,
      amendments: state.amendments + 1,
    }
  }
  if (source.kind !== 'user') return state
  const text = messageText(event.data)
  if (state.text !== null) {
    if (refreezeMinChars === undefined || text.length < Math.max(refreezeMinChars, minChars)) return state
    if (text === state.text) return state
  } else if (text.length < minChars) {
    return state
  }
  return { text, digest: digestOf(text), chars: text.length, lockedAt: event.time, amendedAt: null, amendments: 0 }
}

/**
 * Whether the frozen spec is among the messages a request would send. Callers
 * pass the derived history plus the messages the step proposes, because either
 * end of the request can hold it. A restatement counts by its recorded digest,
 * which is what lets a long spec travel as a pointer instead of a second copy.
 * @param messages - the user-role messages of the request.
 * @param state - the current spec state.
 * @returns true when the request already carries the current spec.
 */
export function specPresentIn(messages: readonly Message[], state: SpecLockState): boolean {
  if (state.text === null) return true
  return messages.some((message): boolean => {
    if (message.role !== 'user') return false
    if (message.source.kind === SPEC_LOCK_SOURCE) return message.source.specDigest === state.digest
    return messageText(message) === state.text
  })
}

/** One restatement of the frozen spec, ready to become a durable user message. */
export interface SpecRestatement {
  /** The message text. */
  readonly text: string
  /** The message source, carrying the digest the restatement is of. */
  readonly source: SpecLockSource
}

/** Header of a restatement that carries the spec text inside itself. */
function restatedHeader(digest: string): string {
  return `Frozen session spec (${digest}), restated because it left the visible history.`
    + ' It stays authoritative; change it only through spec_amend with the user\'s approval.'
}

/**
 * Restate the frozen spec for a message that must carry it back into the
 * request. A spec within `maxInlineChars` is restated in full — a truncated spec
 * invites compliance with text the user never wrote — while a longer one is
 * pointed at by digest and length, and `spec_get` serves its exact text.
 * @param state - the locked spec state.
 * @param maxInlineChars - longest spec restated in full.
 * @returns the restatement text and its source.
 * @throws when the state holds no spec.
 */
export function restateSpec(state: SpecLockState, maxInlineChars: number): SpecRestatement {
  const { text, digest, chars } = state
  if (text === null || digest === null) throw new Error('spec-lock: cannot restate a spec that was never frozen')
  if (chars <= maxInlineChars) {
    return {
      text: `${restatedHeader(digest)}\n\n${text}`,
      source: { kind: SPEC_LOCK_SOURCE, specDigest: digest },
    }
  }
  const pointer = `Frozen session spec (${digest}, ${String(chars)} characters) left the visible history.`
    + ' Call spec_get before continuing; it stays authoritative and unchanged.'
  return {
    text: pointer,
    source: {
      kind: SPEC_LOCK_SOURCE,
      specDigest: digest,
      form: 'notice',
      summary: boundContextSummary(pointer),
    },
  }
}

/** The `specLock` unit with its view present — the shape `register` demands of a client-visible key. */
export type SpecLockProjection = ProjectionDefinition<'specLock', SpecLockState>
  & { wire: NonNullable<ProjectionDefinition<'specLock', SpecLockState>['wire']> }

/**
 * The `specLock` projection: one session's spec folded out of its own history.
 * @param minChars - shortest text a message may freeze with.
 * @param refreezeMinChars - shortest later message that replaces the spec; undefined never replaces it.
 * @returns the definition to register.
 */
export function specLockProjection(minChars: number, refreezeMinChars?: number): SpecLockProjection {
  // The change feed compares raw view results with Object.is, so one state must
  // render one object; folding identity is what memoizes it.
  const views = new WeakMap<SpecLockState, SpecLockView>()
  return {
    key: 'specLock',
    // Version 2: later substantial prompts re-freeze the spec, so a state
    // cached under the first-message-only fold must be recomputed.
    stateVersion: 2,
    stateSchema: specLockStateSchema,
    init: () => emptySpecLockState(),
    apply: (state, event) => applySpecLockEvent(state, event, minChars, refreezeMinChars),
    wire: {
      viewSchema: specLockViewSchema,
      view: (state) => {
        const existing = views.get(state)
        if (existing !== undefined) return existing
        const view: SpecLockView = {
          locked: state.text !== null,
          digest: state.digest,
          chars: state.chars,
          amendments: state.amendments,
        }
        views.set(state, view)
        return view
      },
    },
  }
}
