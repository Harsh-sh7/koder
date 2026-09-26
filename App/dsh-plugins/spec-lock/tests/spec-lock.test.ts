/**
 * The frozen spec is read back out of the durable log, so these tests drive the
 * fold the way a session does — by appending real events — and pin the three
 * decisions that matter: what may freeze (only a substantial user-authored
 * message), what may replace it (only an approved amendment), and when a
 * restatement is owed (only while the spec is absent from the request).
 *
 * The projection is registered beside the real registry so its wire view is
 * validated and its change feed observed as a client sees them; narrow fold
 * paths with an envelope no producer writes (an empty amendment) run against
 * the exported definition directly, as in-repo projection tests do.
 */

import { strict as assert } from 'node:assert'
import { test } from 'node:test'

import { Context } from '@deepseek-ai/cordis'
import { createUserMessage } from '@deepseek-ai/dsh-llm'
import type { UserMessage } from '@deepseek-ai/dsh-llm'
import SessionStore, { SessionId, SessionSeq } from '@deepseek-ai/dsh-session'
import type { Session, SessionEvent } from '@deepseek-ai/dsh-session'
import SessionProjectionRegistry from '@deepseek-ai/dsh-session-projection'

import {
  Config,
  DEFAULT_MAX_INLINE_CHARS,
  DEFAULT_MIN_CHARS,
  SPEC_LOCK_SOURCE,
  applySpecLockEvent,
  digestOf,
  emptySpecLockState,
  messageText,
  restateSpec,
  specLockProjection,
  specPresentIn,
} from '../src/index.ts'
import type { SpecLockState, SpecLockView } from '../src/index.ts'

/** A message the session's participants wrote (a real user prompt). */
function userAuthored(text: string): UserMessage {
  return createUserMessage({ content: [{ type: 'text', text }], source: { kind: 'user' } })
}

/** A message another producer injected into the request. */
function injected(text: string): UserMessage {
  return createUserMessage({ content: [{ type: 'text', text }], source: { kind: 'system-prompt' } })
}

/** A restatement this plugin would write for `digest`. */
function restatement(text: string, digest: string): UserMessage {
  return createUserMessage({ content: [{ type: 'text', text }], source: { kind: SPEC_LOCK_SOURCE, specDigest: digest } })
}

/** An approved amendment of exactly `text`. */
function amendment(text: string): UserMessage {
  return createUserMessage({
    content: [{ type: 'text', text }],
    source: { kind: SPEC_LOCK_SOURCE, specDigest: digestOf(text), specAmendment: true },
  })
}

/** One session with the real store and registry, and the spec lock folded over it. */
async function harness(minChars = DEFAULT_MIN_CHARS): Promise<{ ctx: Context; session: Session }> {
  const ctx = new Context()
  await ctx.plugin(SessionStore)
  await ctx.plugin(SessionProjectionRegistry)
  ctx.sessionProjections.register(specLockProjection(minChars))
  return { ctx, session: ctx.sessions.create(SessionId('spec-locked')) }
}

/** Append one user message under any producer, as the log stores it. */
function append(session: Session, message: UserMessage): void {
  session.append('user/message', message, { surfaceOp: 'append' })
}

/** The fold state the registry serves for one session. */
function stateOf(ctx: Context, session: Session): SpecLockState {
  const state = ctx.sessionProjections.stateOf(session, 'specLock')
  assert.ok(state !== undefined, 'the specLock unit must be registered')
  return state
}

/** The wire view the registry serves for one session. */
function viewOf(ctx: Context, session: Session): SpecLockView {
  return ctx.sessionProjections.snapshot(session).values.specLock as SpecLockView
}

test('the first substantial user-authored message freezes as the spec', async () => {
  const { ctx, session } = await harness()
  append(session, userAuthored('Build me a terminal harness with a token meter.'))
  append(session, userAuthored('and make it fast'))
  const state = stateOf(ctx, session)
  assert.equal(state.text, 'Build me a terminal harness with a token meter.')
  assert.equal(state.digest, digestOf(state.text))
  assert.equal(state.chars, 47)
  assert.ok((state.lockedAt ?? 0) > 0)
  assert.equal(state.amendedAt, null)
  assert.equal(state.amendments, 0)
})

test('context this plugin and others inject cannot become the spec', async () => {
  const { ctx, session } = await harness()
  append(session, injected('a system prompt section'))
  append(session, restatement('Frozen session spec (sha256:000000000000), restated.', 'sha256:000000000000'))
  append(session, userAuthored('the actual request'))
  assert.equal(stateOf(ctx, session).text, 'the actual request')
})

test('a message below minChars does not freeze, and a later substantial one does', async () => {
  const { ctx, session } = await harness(10)
  append(session, userAuthored('ok'))
  assert.equal(stateOf(ctx, session).text, null)
  append(session, userAuthored('now do the real work'))
  assert.equal(stateOf(ctx, session).text, 'now do the real work')
})

test('a whitespace-only message never freezes, even at the loosest bound', async () => {
  const { ctx, session } = await harness()
  append(session, userAuthored('   \n '))
  assert.equal(stateOf(ctx, session).text, null)
})

test('an approved amendment replaces the spec and is counted; restatements never do', async () => {
  const { ctx, session } = await harness()
  const original = 'original spec'
  append(session, userAuthored(original))
  const frozen = stateOf(ctx, session)
  append(session, restatement('Frozen session spec, restated because it left the visible history.', frozen.digest ?? ''))
  assert.equal(stateOf(ctx, session).text, original)
  append(session, amendment('amended spec'))
  const amended = stateOf(ctx, session)
  assert.equal(amended.text, 'amended spec')
  assert.equal(amended.digest, digestOf('amended spec'))
  assert.equal(amended.lockedAt, frozen.lockedAt)
  assert.ok(amended.amendedAt !== null)
  assert.equal(amended.amendments, 1)
})

test('an amendment whose text is empty leaves the spec alone (fabricated envelope)', () => {
  const base: SpecLockState = {
    text: 'kept', digest: digestOf('kept'), chars: 4, lockedAt: 1, amendedAt: null, amendments: 0,
  }
  const event = {
    type: 'user/message',
    seq: SessionSeq(0),
    time: 2,
    data: amendment('   '),
  } as unknown as SessionEvent
  assert.equal(applySpecLockEvent(base, event, DEFAULT_MIN_CHARS), base)
})

test('events of any other type leave the state by reference (fabricated envelope)', () => {
  const base: SpecLockState = {
    text: 'kept', digest: digestOf('kept'), chars: 4, lockedAt: 1, amendedAt: null, amendments: 0,
  }
  const event = { type: 'turn/start', seq: SessionSeq(0), time: 2, data: { turn: 1 } } as unknown as SessionEvent
  assert.equal(applySpecLockEvent(base, event, DEFAULT_MIN_CHARS), base)
})

test('the change feed stays quiet for every event but the freeze and the amendments', async () => {
  const { ctx, session } = await harness()
  const changes: SessionSeq[] = []
  ctx.sessionProjections.onChanged((_session, key, _value, seq) => {
    if (key === 'specLock') changes.push(seq)
  })
  append(session, userAuthored('freeze me'))
  const freezeSeq = SessionSeq(session.seq - 1)
  append(session, userAuthored('a later human message'))
  append(session, restatement('Frozen session spec, restated.', digestOf('freeze me')))
  append(session, amendment('now amended'))
  assert.deepEqual(changes, [freezeSeq, SessionSeq(session.seq - 1)])
})

test('the spec counts as present by its exact text and by its recorded digest', async () => {
  const { ctx, session } = await harness()
  append(session, userAuthored('ship the harness'))
  const state = stateOf(ctx, session)
  const digest = digestOf('ship the harness')
  assert.equal(specPresentIn([injected('other context')], state), false)
  assert.equal(specPresentIn([injected('other context'), restatement('ship the harness', digest)], state), true)
  assert.equal(specPresentIn([createUserMessage({ content: [{ type: 'text', text: 'ship the harness' }], source: { kind: 'user' } })], state), true)
  // A restatement of an older spec does not stand in for the current one.
  assert.equal(specPresentIn([restatement('ship the harness', 'sha256:000000000000')], state), false)
})

test('an unfrozen session always counts as present, so nothing is restated', () => {
  assert.equal(specPresentIn([injected('anything')], emptySpecLockState()), true)
})

test('a short spec is restated in full, with its digest and the amendment rule', async () => {
  const { ctx, session } = await harness()
  append(session, userAuthored('keep it small'))
  const restated = restateSpec(stateOf(ctx, session), DEFAULT_MAX_INLINE_CHARS)
  assert.match(restated.text, /^Frozen session spec \(sha256:[0-9a-f]{12}\)/)
  assert.match(restated.text, /spec_amend/)
  assert.match(restated.text, /keep it small$/)
  assert.equal(restated.source.kind, SPEC_LOCK_SOURCE)
  assert.equal(restated.source.specDigest, stateOf(ctx, session).digest)
  assert.equal(restated.source.form, undefined)
})

test('a long spec is restated as a pointer whose notice summary stays bounded', async () => {
  const { ctx, session } = await harness()
  const long = `Do all of this:\n${'x'.repeat(4_000)}`
  append(session, userAuthored(long))
  const restated = restateSpec(stateOf(ctx, session), DEFAULT_MAX_INLINE_CHARS)
  assert.equal(restated.text.includes('x'.repeat(10)), false)
  assert.match(restated.text, /Call spec_get/)
  assert.equal(restated.source.form, 'notice')
  if (restated.source.form === 'notice') assert.ok(restated.source.summary.length <= 120)
})

test('restating an unfrozen spec is refused', () => {
  assert.throws(() => restateSpec(emptySpecLockState(), DEFAULT_MAX_INLINE_CHARS), /never frozen/u)
})

test('the digest names the text and nothing else', () => {
  assert.equal(digestOf('a'), digestOf('a'))
  assert.notEqual(digestOf('a'), digestOf('b'))
  assert.match(digestOf('a'), /^sha256:[0-9a-f]{12}$/)
})

test('messageText joins text blocks and drops everything else', () => {
  const message = createUserMessage({
    content: [
      { type: 'text', text: 'first' },
      { type: 'reasoning', text: 'not spec text' },
      { type: 'text', text: 'second' },
    ],
    source: { kind: 'user' },
  })
  assert.equal(messageText(message), 'first\nsecond')
})

test('the projection serves one stable view per state and the registry validates it', async () => {
  const { ctx, session } = await harness()
  assert.deepEqual(viewOf(ctx, session), { locked: false, digest: null, chars: 0, amendments: 0 })
  append(session, userAuthored('freeze me'))
  const view = viewOf(ctx, session)
  assert.deepEqual(view, { locked: true, digest: digestOf('freeze me'), chars: 9, amendments: 0 })
  const projection = specLockProjection(DEFAULT_MIN_CHARS)
  const state = stateOf(ctx, session)
  // The change feed compares raw views with Object.is, so one state must render
  // one object.
  assert.equal(projection.wire.view(state), projection.wire.view(state))
  assert.deepEqual(projection.wire.view(state), view)
})

test('the configuration defaults to restating every non-empty spec in full', () => {
  const resolved = new Config({}) as Config
  assert.equal(resolved.maxInlineChars, DEFAULT_MAX_INLINE_CHARS)
  assert.equal(resolved.minChars, DEFAULT_MIN_CHARS)
})
