/**
 * The harness composition: one operating contract every step carries, two tools
 * that make parallel work and independent review mechanical, and a budget the
 * harness enforces rather than the model.
 *
 * The plugin owns three seams and no state of its own beyond the live session:
 *
 * - A prompt section (`harness:operating-contract`) states the invariants the
 *   rest of the composition enforces. It is constant text on purpose — a section
 *   that changed per step would break the prompt cache for the whole prefix.
 * - `wave_plan` partitions a decomposed job into file-disjoint waves, and
 *   `committee` seats independent reviewers on a decision no runnable check can
 *   settle. Both replace multi-turn model bookkeeping with one deterministic
 *   call, which is where their token win is: `wave_plan` is arithmetic the model
 *   would otherwise re-derive per step, and `committee` is one tool result
 *   instead of a spawn/wait/tally cycle.
 * - An `agent/pre-step` listener counts model tokens and turn steps against the
 *   configured ceiling. At the ceiling it asks the human — through the approval
 *   channel, the only human channel the ACP bridge carries — and only an
 *   explicit grant buys more steps; anything else ends the turn (`blocked`),
 *   which is a stop the model cannot argue with.
 *
 * An orchestration skill (`skills/orchestrate.md`) carries the long-form
 * procedure, advertised as one line in the skill catalog and loaded only when a
 * job is bigger than one edit.
 *
 * @module @harness/dsh-hackathon-harness-preset
 */

import { readFileSync } from 'node:fs'

import { brandString } from '@deepseek-ai/dsh-brand'
import type { PreStepDecision } from '@deepseek-ai/dsh-agent'
import type { Context } from '@deepseek-ai/cordis'
import { boundContextSummary, createUserMessage } from '@deepseek-ai/dsh-llm'
import type { ContextFormed, ToolCallId } from '@deepseek-ai/dsh-llm'
import type { Session } from '@deepseek-ai/dsh-session'
import type {} from '@deepseek-ai/dsh-session-projection'
import type {} from '@deepseek-ai/dsh-skill'
import type { SubagentResult, SubagentRun } from '@deepseek-ai/dsh-subagent'
import type {} from '@deepseek-ai/dsh-subagent'
import type {} from '@deepseek-ai/dsh-system-prompt'
import type {} from '@deepseek-ai/dsh-token-meter'
import { defineTool } from '@deepseek-ai/dsh-tools'
import type { ObjectJsonSchema } from '@deepseek-ai/dsh-tools'
import type {} from '@deepseek-ai/dsh-user-approval'
import z from '@deepseek-ai/schemastery'
import { decideBudget, formatCount, totalTokens } from './budget.ts'
import {
  COMMITTEE_DESCRIPTION,
  CONTRACT_SECTION,
  committeePrompt,
  contractText,
  REVIEW_ROLES,
  VERDICT_SCHEMA,
  WAVE_PLAN_DESCRIPTION,
} from './policy.ts'
import type { ReviewRole } from './policy.ts'
import { planWaves } from './waves.ts'
import type { Subtask, WavePlan } from './waves.ts'

export { decideBudget, formatCount, totalTokens } from './budget.ts'
export type { BudgetDecision, BudgetLimits, BudgetUsage, TokenTotals } from './budget.ts'
export { COMMITTEE_DESCRIPTION, CONTRACT_SECTION, contractText, REVIEW_ROLES, WAVE_PLAN_DESCRIPTION } from './policy.ts'
export type { ReviewRole } from './policy.ts'
export { normalizePath, planWaves, WavePlanError } from './waves.ts'
export type { FileConflict, Subtask, Wave, WavePlan } from './waves.ts'

declare module '@deepseek-ai/dsh-llm' {
  interface MessageSourceMap {
    'harness-budget': { kind: 'harness-budget' } & ContextFormed
  }
}

/** Plugin name as the loader reports it. */
export const name = 'harness-preset'
/** The seams this plugin contributes through. `subagents` and `approval` are read lazily: the contract holds without them. */
export const inject = ['tools', 'systemPrompt']

/** The partitioner tool. */
export const WAVE_PLAN = 'wave_plan'
/** The reviewer tool. */
export const COMMITTEE = 'committee'
/**
 * Tool name the budget escalation is filed under, and the call id its approval
 * prompt carries. Both are conventions a client can present deliberately: the
 * ACP bridge answers a permission request only when the asker named a call, so
 * the budget asks under this fixed identity rather than a phantom call id.
 */
export const BUDGET_TOOL = 'harness_budget'

/** Name of the model-facing orchestration skill. */
export const ORCHESTRATION_SKILL = 'orchestrate'

/** Skill catalogue line: what the skill is for, in the router's words. */
const ORCHESTRATION_SKILL_DESCRIPTION = 'Decompose a multi-part job into file-disjoint waves, prove each part before building it,'
  + ' convene a review committee for work no check can settle, and land the budget. Load it when a job spans several files'
  + ' or several parts.'

/** Default ceiling: model tokens one session may spend before the harness stops it. */
export const DEFAULT_MAX_TOKENS = 400_000
/** Default cap on model steps in one turn. */
export const DEFAULT_STEPS_PER_TURN = 60
/** Default fraction of the ceiling at which the model is warned once. */
export const DEFAULT_WARN_AT = 0.8
/** Default number of approved escalations one session may take. */
export const DEFAULT_MAX_ESCALATIONS = 2

/** Plugin configuration. */
export interface Config {
  /** Model-token ceiling for one session; each approved escalation raises the live ceiling. Defaults to 400000. */
  maxTokens?: number
  /** Model steps one turn may take. Defaults to 60. */
  stepsPerTurn?: number
  /** Fraction of the ceiling at which the model is warned once. Defaults to 0.8. */
  warnAt?: number
  /** Tokens one approved escalation adds. Defaults to half the ceiling. */
  grantTokens?: number
  /** Approved escalations allowed per session. Defaults to 2. */
  maxEscalations?: number
  /** Whether the operating-contract section is contributed. Defaults to true. */
  contract?: boolean
  /** Whether the orchestration skill is registered. Defaults to true. */
  orchestrationSkill?: boolean
  /** `ctx.subagents` provider the committee seats its reviewers on. Defaults to `spawn` (a fresh child per reviewer). */
  committeeProvider?: string
  /** Role ids of the default committee bench, in seating order. Defaults to every role in {@link REVIEW_ROLES}. */
  committeeRoles?: string[]
  /** Tool mask applied to committee children; omitted means they inherit every tool. */
  committeeTools?: { allow?: string[]; deny?: string[] }
}

/** Runtime schema for {@link Config}. */
export const Config = z.object({
  maxTokens: z.number().step(1).min(1).default(DEFAULT_MAX_TOKENS),
  stepsPerTurn: z.number().step(1).min(1).default(DEFAULT_STEPS_PER_TURN),
  warnAt: z.number().min(0).max(1).default(DEFAULT_WARN_AT),
  grantTokens: z.number().step(1).min(1),
  maxEscalations: z.number().step(1).min(0).default(DEFAULT_MAX_ESCALATIONS),
  contract: z.boolean().default(true),
  orchestrationSkill: z.boolean().default(true),
  committeeProvider: z.string().min(1).default('spawn'),
  committeeRoles: z.array(z.string()).default(REVIEW_ROLES.map(role => role.id)),
  committeeTools: z.union([
    z.object({ allow: z.array(z.string()), deny: z.array(z.string()) }),
    z.const(undefined),
  ]),
})

/** One session's live budget state: what the ceiling currently is and what it has already done. */
interface CapState {
  /** Tokens the session may spend, including every granted escalation. */
  limitTokens: number
  /** Escalations the user has approved for this session. */
  escalations: number
  /** The ceiling a warning was last issued for, so one ceiling warns once. */
  warnedAtLimit: number | undefined
}

/** The budget warning, admitted into the step that crossed the threshold. */
function budgetMessage(text: string) {
  return createUserMessage({
    content: [{ type: 'text', text }],
    source: { kind: 'harness-budget', form: 'notice', summary: boundContextSummary(text) },
  })
}

/**
 * Resolve the configured bench into roles, refusing an unknown id loudly: a
 * typo that silently shrank a bench would turn a quorum into a majority of one.
 * @param ids - configured role ids, in seating order.
 * @returns the roles to seat.
 * @throws when a configured id names no role.
 */
function resolveRoles(ids: readonly string[]): ReviewRole[] {
  if (ids.length === 0) throw new Error('harness-preset: committeeRoles must name at least one reviewer role')
  return ids.map((id) => {
    const role = REVIEW_ROLES.find(candidate => candidate.id === id)
    if (role === undefined) {
      throw new Error(`harness-preset: unknown committee role "${id}"; known roles:`
        + ` ${REVIEW_ROLES.map(candidate => candidate.id).join(', ')}`)
    }
    return role
  })
}

/** Readable text for a thrown value, which a tool result can carry into a report. */
function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** One seated reviewer's outcome: a terminal result, or the fault that replaced it. */
interface SeatOutcome {
  /** The reviewer's terminal result, present when the run settled. */
  readonly result?: SubagentResult
  /** The fault that replaced a result, present when seating or the run itself failed. */
  readonly failure?: string
}

/**
 * Why one seated reviewer contributed no countable verdict. A reviewer that
 * returns an unparseable verdict is not an approval, so the reason has to name
 * which of the three ways the seat went quiet rather than collapse into silence.
 * @param outcome - the seat's outcome, absent when the seat was never established.
 * @returns one line naming the failure.
 */
function verdictFailure(outcome: SeatOutcome | undefined): string {
  if (outcome === undefined) return 'the seat was never established'
  if (outcome.failure !== undefined) return outcome.failure
  const result = outcome.result
  if (result === undefined) return 'the reviewer settled without a result'
  if (result.stopReason !== 'completed') {
    return `${result.stopReason}${result.diagnostic === undefined ? '' : `: ${result.diagnostic}`}`
  }
  return 'the reviewer returned no verdict this harness could count'
}

/** The plan as the model reads it. */
function renderPlan(plan: WavePlan): string {
  const lines = [`${plan.waves.length === 1 ? '1 wave' : `${String(plan.waves.length)} waves`} (depth ${String(plan.depth)}):`]
  for (const wave of plan.waves) {
    lines.push(`  wave ${String(wave.index)}${wave.parallel ? ' (parallel)' : ''}: ${wave.subtasks.join(', ')}`)
  }
  if (plan.conflicts.length > 0) {
    lines.push('forced ordering — these files are declared by more than one subtask:')
    for (const conflict of plan.conflicts) lines.push(`  ${conflict.file}: ${conflict.subtasks.join(', ')}`)
  }
  if (plan.undeclared.length > 0) {
    lines.push(`declared no files, so the plan could not check them: ${plan.undeclared.join(', ')}`)
  }
  return lines.join('\n')
}

/**
 * Register the harness composition for the lifetime of `ctx`.
 * @param ctx - plugin context carrying the tool and prompt seams.
 * @param config - budget ceilings, bench, and tool masks.
 */
export function apply(ctx: Context, config: Config): void {
  const maxTokens = config.maxTokens ?? DEFAULT_MAX_TOKENS
  const stepsPerTurn = config.stepsPerTurn ?? DEFAULT_STEPS_PER_TURN
  const warnAt = config.warnAt ?? DEFAULT_WARN_AT
  const grantTokens = config.grantTokens ?? Math.floor(maxTokens / 2)
  const maxEscalations = config.maxEscalations ?? DEFAULT_MAX_ESCALATIONS
  const provider = config.committeeProvider ?? 'spawn'
  const bench = resolveRoles(config.committeeRoles ?? REVIEW_ROLES.map(role => role.id))
  const tools = config.committeeTools
  const caps = new WeakMap<Session, CapState>()

  if (config.contract ?? true) {
    ctx.systemPrompt.section({
      name: CONTRACT_SECTION,
      order: ctx.systemPrompt.getSectionOrder('TEAM_POLICY'),
      text: contractText(),
    })
  }

  if (config.orchestrationSkill ?? true) {
    // The body lives beside the plugin as Markdown so it stays prose a reviewer
    // can edit; only the routing line is code.
    const path = new URL('../skills/orchestrate.md', import.meta.url)
    let content: string
    try {
      content = readFileSync(path, 'utf8')
    } catch (error: unknown) {
      throw new Error(`harness-preset: cannot read ${path.pathname}: the orchestration skill ships with the plugin`, { cause: error })
    }
    ctx.inject(['skills'], (skillsCtx: Context) => {
      skillsCtx.skills.register({
        name: ORCHESTRATION_SKILL,
        description: ORCHESTRATION_SKILL_DESCRIPTION,
        // The registry's runtime bucket: this body ships inside a plugin rather
        // than in a user or project skill directory.
        source: 'runtime',
        content,
      })
    })
  }

  ctx.tools.register(defineTool({
    name: WAVE_PLAN,
    description: WAVE_PLAN_DESCRIPTION,
    parameters: {
      subtasks: {
        type: 'array',
        required: true,
        description: 'Every part of the job, each naming the files it will write.',
        items: {
          type: 'object',
          additionalProperties: false,
          properties: {
            id: { type: 'string', required: true, description: 'Short stable name for this part, unique in the list.' },
            files: {
              type: 'array',
              required: true,
              items: { type: 'string' },
              description: 'Workspace-relative files this part will write. Leave empty only when it truly writes none.',
            },
          },
        },
      },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          depth: { type: 'number', required: true },
          waves: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                index: { type: 'number', required: true },
                parallel: { type: 'boolean', required: true },
                subtasks: { type: 'array', required: true, items: { type: 'string' } },
              },
            },
          },
          conflicts: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                file: { type: 'string', required: true },
                subtasks: { type: 'array', required: true, items: { type: 'string' } },
              },
            },
          },
          undeclared: { type: 'array', required: true, items: { type: 'string' } },
        },
      },
      render: (_args, value) => [{ type: 'text', text: renderPlan(value) }],
    },
    execute: async (args) => {
      const subtasks: Subtask[] = args.subtasks.map(part => ({ id: part.id, files: part.files }))
      return planWaves(subtasks)
    },
    presentCall: () => ({ card: 'generic', title: 'Plan parallel waves', kind: 'read' }),
    presentResult: () => ({ card: 'generic', title: 'Wave plan' }),
  }))

  ctx.tools.register(defineTool({
    name: COMMITTEE,
    description: COMMITTEE_DESCRIPTION,
    parameters: {
      decision: {
        type: 'string',
        required: true,
        description: 'The one claim under review, stated so it could be falsified — not the whole change.',
      },
      evidence: {
        type: 'string',
        description: 'Paths, commands, or observed output the reviewers should work from. Prefer pointing at the workspace over pasting.',
      },
      roles: {
        type: 'array',
        items: { type: 'string' },
        description: `Role ids to seat, defaulting to the deployment bench (${REVIEW_ROLES.map(role => role.id).join(', ')}).`,
      },
      quorum: {
        type: 'number',
        description: 'Approvals needed to carry the decision; defaults to a majority of the seated bench.',
      },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          passed: { type: 'boolean', required: true },
          needed: { type: 'number', required: true },
          approve: { type: 'number', required: true },
          reject: { type: 'number', required: true },
          verdicts: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                role: { type: 'string', required: true },
                vote: { type: 'string', required: true },
                reason: { type: 'string', required: true },
              },
            },
          },
          failures: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                role: { type: 'string', required: true },
                detail: { type: 'string', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => {
        const lines = value.verdicts.map(verdict => `${verdict.role}: ${verdict.vote} — ${verdict.reason}`)
        for (const failure of value.failures) lines.push(`${failure.role}: no verdict — ${failure.detail}`)
        lines.push('', value.passed
          ? `Quorum carried: ${String(value.approve)} of ${String(value.approve + value.reject)} seated reviewers approved`
            + ` (${String(value.needed)} needed).`
          : `Quorum did not carry: ${String(value.approve)} of ${String(value.approve + value.reject)} seated reviewers approved`
            + ` (${String(value.needed)} needed). Treat every rejection as a finding and correct the subtask.`)
        return [{ type: 'text', text: lines.join('\n') }]
      },
    },
    execute: async (args, exec) => {
      const agent = exec.agent
      if (agent === undefined) {
        throw new Error(`${COMMITTEE} requires a calling agent (there is no session whose workspace reviewers could read)`)
      }
      const subagents = ctx.get('subagents')
      if (subagents === undefined) {
        throw new Error(`${COMMITTEE}: no subagent service is composed, so no reviewer can be seated;`
          + ' verify the decision yourself and say in your report which check you ran')
      }
      const decision = args.decision.trim()
      if (decision.length === 0) throw new Error(`${COMMITTEE}: the decision under review must be non-empty`)
      const seated = args.roles === undefined || args.roles.length === 0
        ? bench
        : resolveRoles(args.roles)
      const needed = args.quorum ?? Math.floor(seated.length / 2) + 1
      if (needed < 1 || needed > seated.length) {
        throw new Error(`${COMMITTEE}: quorum must be between 1 and the ${String(seated.length)} seated reviewers`)
      }
      const starts = await Promise.allSettled(seated.map(role => subagents.start(provider, {
        label: `${COMMITTEE}:${role.id}`,
        prompt: [{ type: 'text', text: committeePrompt(role, decision, args.evidence) }],
        parent: agent,
        signal: exec.signal,
        persona: role.persona,
        outputSchema: VERDICT_SCHEMA as unknown as ObjectJsonSchema,
        // A reviewer reviews: one level of delegation is enough to read and run
        // checks, and a reviewer that spawns a bench of its own is pure cost.
        maxDepth: 1,
        ...tools === undefined ? {} : { toolFilter: tools },
      })))
      // Every seat is attempted before any is awaited, so the bench reviews in
      // parallel and the parent pays one round trip for the whole committee.
      const runs = new Map<string, SubagentRun>()
      const failures: { role: string; detail: string }[] = []
      starts.forEach((settled, index) => {
        const role = seated[index]
        if (role === undefined) return
        if (settled.status === 'fulfilled') runs.set(role.id, settled.value)
        else failures.push({ role: role.id, detail: describeError(settled.reason) })
      })
      // A seat that failed is one fewer reviewer, and an unreachable reviewer
      // never counts as an approval — so a bench too small to carry the quorum
      // is refused here, releasing whatever did seat, instead of spending every
      // seated reviewer's tokens on a tally that cannot pass.
      if (runs.size < needed) {
        await Promise.allSettled([...runs.values()].map(run => run.dispose()))
        throw new Error(`${COMMITTEE}: ${String(runs.size)} of ${String(seated.length)} reviewers could not be seated,`
          + ` and ${String(needed)} approvals are needed:`
          + ` ${failures.map(failure => `${failure.role} (${failure.detail})`).join('; ')}`)
      }
      const outcomes = new Map(await Promise.all([...runs.entries()].map(
        async ([roleId, run]): Promise<[string, SeatOutcome]> => {
          try {
            return [roleId, { result: await run.result }]
          } catch (error: unknown) {
            // `result` rejects only on an infrastructure fault the seam cannot
            // name as a stop reason; either way the seat yields no verdict.
            return [roleId, { failure: describeError(error) }]
          } finally {
            // Cleanup must not discard a verdict the reviewer already produced.
            await run.dispose().catch((error: unknown) => {
              ctx.logger.warn(`harness-preset: ${COMMITTEE}: releasing reviewer ${roleId} failed: ${describeError(error)}`)
            })
          }
        },
      )))
      const verdicts: { role: string; vote: string; reason: string }[] = []
      for (const role of seated) {
        if (!runs.has(role.id)) continue
        const outcome = outcomes.get(role.id)
        const structured = outcome?.result?.structured as { vote?: unknown; reason?: unknown } | undefined
        const vote = structured?.vote
        if (outcome?.result?.stopReason === 'completed' && (vote === 'approve' || vote === 'reject')) {
          verdicts.push({
            role: role.id,
            vote,
            reason: typeof structured?.reason === 'string' && structured.reason.trim().length > 0
              ? structured.reason.trim()
              : '(the reviewer gave no reason)',
          })
          continue
        }
        failures.push({ role: role.id, detail: verdictFailure(outcome) })
      }
      // A failure makes the quorum harder to carry without ever counting as a
      // rejection: the model reads which reviewers were silent and why.
      const approve = verdicts.filter(verdict => verdict.vote === 'approve').length
      return {
        passed: approve >= needed,
        needed,
        approve,
        reject: verdicts.length - approve,
        verdicts,
        failures,
      }
    },
    presentCall: () => ({ card: 'generic', title: 'Convene a review committee', kind: 'read' }),
    presentResult: () => ({ card: 'generic', title: 'Committee verdict' }),
  }))

  ctx.on('agent/pre-step', async ({ agent, step, signal }, next): Promise<PreStepDecision> => {
    const decision = await next()
    if (decision.kind === 'reject' || signal.aborted) return decision
    const session = agent.session
    let cap = caps.get(session)
    if (cap === undefined) {
      cap = { limitTokens: maxTokens, escalations: 0, warnedAtLimit: undefined }
      caps.set(session, cap)
    }
    const projections = ctx.get('sessionProjections')
    const usage = projections?.stateOf(session, 'tokenUsage')
    const verdict = decideBudget(
      { tokens: cap.limitTokens, stepsPerTurn, warnAt },
      {
        // A composition without the token meter cannot measure spend, so only
        // the step cap can fire there; the ceiling still governs escalation.
        tokens: usage === undefined ? 0 : totalTokens(usage.totals),
        stepsInTurn: step,
      },
    )
    if (verdict.kind === 'ok') return decision
    if (verdict.kind === 'warn') {
      if (cap.warnedAtLimit === cap.limitTokens) return decision
      cap.warnedAtLimit = cap.limitTokens
      return { ...decision, messages: [...decision.messages, budgetMessage(verdict.text)] }
    }
    const approval = ctx.get('approval')
    const root = session.header.parentSession === undefined
    if (approval !== undefined && root && cap.escalations < maxEscalations) {
      const outcome = await approval.request({
        agent,
        toolName: BUDGET_TOOL,
        callId: brandString<ToolCallId>(BUDGET_TOOL),
        reason: `${verdict.text} Grant ${formatCount(grantTokens)} more model tokens`
          + ` (escalation ${formatCount(cap.escalations + 1)} of ${formatCount(maxEscalations)})?`,
        signal,
      })
      if (outcome === 'allowed-once') {
        cap.limitTokens += grantTokens
        cap.escalations += 1
        ctx.logger.info(`harness-preset: user granted ${String(grantTokens)} more model tokens;`
          + ` the session ceiling is now ${String(cap.limitTokens)}`)
        return decision
      }
      ctx.logger.info(`harness-preset: budget escalation ${outcome}; the turn ends here`)
    }
    ctx.logger.warn(`harness-preset: ${verdict.text}`)
    return { kind: 'reject' }
  })
}
