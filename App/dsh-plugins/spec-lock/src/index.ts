/**
 * Spec lock: a session's opening instruction is frozen as its spec, and the
 * harness keeps that spec visible for the whole session.
 *
 * The spec is identified out of the session's own history — its first
 * substantial user-authored message — so it is durable, forked, and resumed
 * without any plugin-owned event type (see {@link applySpecLockEvent}). What
 * this plugin adds is one invariant: whenever compaction or pruning drops the
 * spec from a request, the next step restates it — in full, or as a digest
 * pointer when it is long — and `spec_get` serves its exact text on demand.
 *
 * Changing the spec is the user's decision, not the model's: `spec_amend` asks
 * the composed approval channel, and only an approved amendment becomes
 * authoritative. Restatements never do.
 *
 * Token posture: while the spec is present in the request this plugin adds
 * nothing but two small tool schemas, and a long spec costs one pointer line
 * instead of a second full copy.
 *
 * @module @harness/dsh-spec-lock
 */

import type { Agent, PreStepDecision } from '@deepseek-ai/dsh-agent'
import type { Context } from '@deepseek-ai/cordis'
import { createUserMessage } from '@deepseek-ai/dsh-llm'
import z from '@deepseek-ai/schemastery'
import type { Session } from '@deepseek-ai/dsh-session'
import { defineTool } from '@deepseek-ai/dsh-tools'
import type {} from '@deepseek-ai/dsh-session-projection'
import type {} from '@deepseek-ai/dsh-user-approval'
import {
  DEFAULT_MAX_INLINE_CHARS,
  DEFAULT_MIN_CHARS,
  SPEC_LOCK_SOURCE,
  digestOf,
  restateSpec,
  specLockProjection,
  specPresentIn,
} from './spec.ts'
import type { SpecLockState } from './spec.ts'

export {
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
} from './spec.ts'
export type { SpecLockSource, SpecLockState, SpecLockView, SpecRestatement } from './spec.ts'

/** Plugin name as the loader reports it. */
export const name = 'spec-lock'
/** The seams this plugin contributes through. */
export const inject = ['tools', 'sessionProjections']

/** Plugin configuration. */
export interface Config {
  /** Longest spec that is restated in full; a longer one is restated as a digest pointer. Defaults to 1500. */
  maxInlineChars?: number
  /** Shortest text a message may freeze as the spec with. Defaults to 1. */
  minChars?: number
}

/** Runtime schema for {@link Config}. */
export const Config: z<Config> = z.object({
  maxInlineChars: z.number().step(1).min(1).default(DEFAULT_MAX_INLINE_CHARS),
  minChars: z.number().step(1).min(1).default(DEFAULT_MIN_CHARS),
})

/** The model-facing read tool. */
export const SPEC_GET = 'spec_get'
/** The approval-gated write tool. */
export const SPEC_AMEND = 'spec_amend'

const GET_DESCRIPTION = 'Read this session\'s frozen spec: the opening instruction kept authoritative across'
  + ' context compaction. Call it when the spec has left the visible history, or before work that must comply with it.'

const AMEND_DESCRIPTION = 'Replace this session\'s frozen spec, after the user approves the change. Call it only when'
  + ' the user asks for the spec itself to change; the approved text becomes authoritative.'

/**
 * Register the spec lock for the lifetime of `ctx`.
 * @param ctx - plugin context carrying the tools and projection seams.
 * @param config - bounds on what freezes and how a long spec is restated.
 */
export function apply(ctx: Context, config: Config): void {
  const minChars = config.minChars ?? DEFAULT_MIN_CHARS
  const maxInlineChars = config.maxInlineChars ?? DEFAULT_MAX_INLINE_CHARS
  ctx.sessionProjections.register(specLockProjection(minChars))

  /** The fold state of one session; the projection is registered by this plugin. */
  const stateOf = (session: Session): SpecLockState => {
    const state = ctx.sessionProjections.stateOf(session, 'specLock')
    if (state === undefined) throw new Error('spec-lock requires the specLock session projection')
    return state
  }

  /** The locked spec, or a refusal naming the tool that needed it. */
  const lockedOf = (session: Session, tool: string): { text: string; digest: string; chars: number } => {
    const state = stateOf(session)
    if (state.text === null || state.digest === null) {
      throw new Error(`${tool}: this session has no frozen spec yet; the spec is its first user message of at`
        + ` least ${String(minChars)} character(s), and none has qualified`)
    }
    return { text: state.text, digest: state.digest, chars: state.chars }
  }

  /** The calling agent, or a refusal: a tool without one has no session to read. */
  const agentOf = (exec: { readonly agent?: Agent }, tool: string): Agent => {
    if (exec.agent === undefined) {
      throw new Error(`${tool} requires a calling agent (there is no session whose spec it could read)`)
    }
    return exec.agent
  }

  // A restatement is a durable user message admitted to the step, exactly how
  // the request itself is assembled: the spec then rides the same path as any
  // other injected context, and a later compaction drops it without losing it.
  ctx.on('agent/pre-step', async ({ agent, signal }, next): Promise<PreStepDecision> => {
    const decision = await next()
    if (decision.kind === 'reject' || signal.aborted) return decision
    const state = stateOf(agent.session)
    if (state.text === null) return decision
    const proposed = decision.messages
    if (specPresentIn([...agent.session.deriveMessages(), ...proposed], state)) return decision
    const restatement = restateSpec(state, maxInlineChars)
    return {
      ...decision,
      messages: [
        ...proposed,
        createUserMessage({
          content: [{ type: 'text', text: restatement.text }],
          source: restatement.source,
        }),
      ],
    }
  })

  ctx.tools.register(defineTool({
    name: SPEC_GET,
    description: GET_DESCRIPTION,
    parameters: {},
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          text: { type: 'string', required: true },
          digest: { type: 'string', required: true },
          chars: { type: 'number', required: true },
        },
      },
      render: (_args, value) => [{ type: 'text', text: value.text }],
    },
    execute: async (_args, exec) => {
      const spec = lockedOf(agentOf(exec, SPEC_GET).session, SPEC_GET)
      return { text: spec.text, digest: spec.digest, chars: spec.chars }
    },
    presentCall: () => ({ card: 'generic', title: 'Read the frozen spec', kind: 'read' }),
    presentResult: () => ({ card: 'generic', title: 'Frozen spec' }),
  }))

  ctx.tools.register(defineTool({
    name: SPEC_AMEND,
    description: AMEND_DESCRIPTION,
    parameters: {
      text: {
        type: 'string',
        required: true,
        description: 'The complete replacement spec, written as it should read from now on.',
      },
      reason: { type: 'string', description: 'Why it is changing; shown to the user in the approval prompt.' },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          digest: { type: 'string', required: true },
          chars: { type: 'number', required: true },
        },
      },
      render: (_args, value) => [{
        type: 'text',
        text: `The user approved the amendment; the frozen spec is now ${String(value.chars)} characters`
          + ` (${value.digest}). The amended text follows and is authoritative.`,
      }],
    },
    execute: async (args, exec) => {
      const spec = lockedOf(agentOf(exec, SPEC_AMEND).session, SPEC_AMEND)
      const text = args.text.trim()
      if (text.length === 0) throw new Error(`${SPEC_AMEND}: the replacement spec must be non-empty`)
      if (text === spec.text) {
        throw new Error(`${SPEC_AMEND}: the replacement text is identical to the frozen spec; there is nothing to amend`)
      }
      const approval = ctx.get('approval')
      if (approval === undefined) {
        throw new Error(`${SPEC_AMEND}: no approval channel is composed, so the user cannot approve a spec change;`
          + ' ask them in your reply instead')
      }
      const outcome = await approval.request({
        agent: agentOf(exec, SPEC_AMEND),
        toolName: SPEC_AMEND,
        callId: exec.callId,
        reason: `${args.reason ?? 'The model proposes amending this session\'s spec.'}`
          + ` (frozen: ${spec.digest}, ${String(spec.chars)} characters; proposed: ${String(text.length)} characters)`,
        signal: exec.signal,
      })
      if (outcome === 'rejected') {
        throw new Error(`${SPEC_AMEND}: the user rejected the amendment; the frozen spec is unchanged —`
          + ' do not act on the proposed text')
      }
      if (outcome === 'cancelled') {
        throw new Error(`${SPEC_AMEND}: the amendment was withdrawn before the user decided; the frozen spec is unchanged`)
      }
      if (outcome === 'unavailable') {
        throw new Error(`${SPEC_AMEND}: no approval channel answered, so the change was not approved;`
          + ' the frozen spec is unchanged')
      }
      const digest = digestOf(text)
      exec.deferContext(createUserMessage({
        content: [{ type: 'text', text }],
        source: { kind: SPEC_LOCK_SOURCE, specDigest: digest, specAmendment: true },
      }))
      return { digest, chars: text.length }
    },
    presentCall: () => ({ card: 'generic', title: 'Amend the frozen spec', kind: 'edit' }),
    presentResult: () => ({ card: 'generic', title: 'Spec amended' }),
  }))
}
