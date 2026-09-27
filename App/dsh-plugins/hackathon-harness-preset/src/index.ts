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
import { decideBudget, formatCount, weightedTokens } from './budget.ts'
import {
  COMMITTEE_DESCRIPTION,
  CONTRACT_SECTION,
  RUN_WAVE_DESCRIPTION,
  committeePrompt,
  contractText,
  REVIEW_ROLES,
  VERDICT_SCHEMA,
  WAVE_PLAN_DESCRIPTION,
} from './policy.ts'
import type { ReviewRole } from './policy.ts'
import { planWaves } from './waves.ts'
import type { Subtask, WavePlan } from './waves.ts'
import { batchParts, ownsPath, PART_RESULT_SCHEMA, partPrompt, renderRun } from './delegate.ts'
import type { PartOutcome, WavePart } from './delegate.ts'
import { buildReport, renderReport } from './review.ts'
import type { BudgetSnapshot, TurnReport, WaveOutcomeLog } from './review.ts'

export { decideBudget, formatCount, totalTokens, weightedTokens } from './budget.ts'
export type { BudgetDecision, BudgetLimits, BudgetUsage, TokenTotals } from './budget.ts'
export { COMMITTEE_DESCRIPTION, CONTRACT_SECTION, contractText, REVIEW_ROLES, WAVE_PLAN_DESCRIPTION } from './policy.ts'
export type { ReviewRole } from './policy.ts'
export { normalizePath, planWaves, WavePlanError } from './waves.ts'
export { batchParts, ownsPath, partPrompt, renderRun } from './delegate.ts'
export type { PartOutcome, WavePart } from './delegate.ts'
export type { FileConflict, Subtask, Wave, WavePlan } from './waves.ts'
export { buildReport, renderReport } from './review.ts'
export type { BudgetSnapshot, Finding, TurnReport, WaveOutcomeLog } from './review.ts'

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
/** The tool that runs a decomposed job's parts as parallel subagents. */
export const RUN_WAVE = 'run_wave'
/**
 * Automatic retries for a `run_wave` part whose model request did not
 * complete (a thrown start, or a `stopReason` other than `completed`).
 *
 * Parts in one wave are started together, which asks several requests of the
 * same route at once — the shape most likely to trip a provider's transient
 * rate limit or concurrent-request throttle, independent of whether the part
 * itself was doable. One retry turns that into a delay instead of a lost
 * part; it is not retried when the part's own agent completed and reported
 * `failed` through `structured_output` — that is a real verdict, not a
 * transport fault, and repeating identical work would not change it.
 */
export const PART_RETRIES = 1
/** How long a retried part waits before starting again. */
export const PART_RETRY_DELAY_MS = 2_000
/** How far apart a wave's parts are staggered on their first attempt. */
export const PART_START_STAGGER_MS = 400
/**
 * Prefix of a live-stream line on stderr. The record-separator control
 * character never begins an ordinary log line, so the application can split
 * these out of the engine's log without guessing.
 */
export const LIVE_STREAM_TAG = '\u001eharness-live '
/** Default milliseconds between batches, when a wave is split by `maxConcurrentParts`. */
export const DEFAULT_INTER_BATCH_DELAY_MS = 20_000
/** Resolves after `ms` milliseconds. */
const sleep = (ms: number): Promise<void> => new Promise(resolve => { setTimeout(resolve, ms) })
/**
 * Tools a `run_wave` part's child never gets: a part is built, not delegated
 * again — nested fan-out would multiply the cost of every part.
 */
export const PART_DENIED_TOOLS: readonly string[] = ['subagent', 'committee', 'wave_plan', 'run_wave', 'spec_amend']
/** Tools that write a file, by the argument naming it; `run_wave` holds these to a part's own files. */
const WRITE_TOOLS: Readonly<Record<string, string>> = { write: 'file_path', edit: 'file_path' }
/**
 * Tool name the budget escalation is filed under, and the call id its approval
 * prompt carries. Both are conventions a client can present deliberately: the
 * ACP bridge answers a permission request only when the asker named a call, so
 * the budget asks under this fixed identity rather than a phantom call id.
 */
export const BUDGET_TOOL = 'harness_budget'
/**
 * The turn self-check tool: a findings report built from the same budget and
 * `run_wave` state the harness already tracks (see `review.ts`). Read-only —
 * it never changes the ceiling, the wave plan, or anything on disk.
 */
export const HARNESS_REPORT = 'harness_report'

/** Name of the model-facing orchestration skill. */
export const ORCHESTRATION_SKILL = 'orchestrate'

/** Skill catalogue line: what the skill is for, in the router's words. */
const ORCHESTRATION_SKILL_DESCRIPTION = 'Decompose a multi-part job into file-disjoint waves, prove each part before building it,'
  + ' convene a review committee for work no check can settle, and land the budget. Load it when a job spans several files'
  + ' or several parts.'

/** Default ceiling: weighted model tokens one budget scope may spend before the harness stops it. */
export const DEFAULT_MAX_TOKENS = 1_500_000
/** Default cap on model steps in one turn. */
export const DEFAULT_STEPS_PER_TURN = 90
/** Default scope of the token ceiling: every prompt (one issue) gets its own. */
export const DEFAULT_BUDGET_SCOPE = 'turn'
/** Default weight of a prompt-cache read against a fresh input token. */
export const DEFAULT_CACHE_READ_WEIGHT = 0.1
/**
 * Tools a committee reviewer never gets: reviewers judge, they do not change
 * the workspace or convene reviewers of their own. Names that are not
 * registered in a composition are skipped rather than refused.
 */
export const REVIEWER_DENIED_TOOLS: readonly string[] = [
  'edit', 'write', 'todo_write', 'subagent', 'committee', 'wave_plan', 'run_wave', 'spec_amend',
]
/** Default fraction of the ceiling at which the model is warned once. */
export const DEFAULT_WARN_AT = 0.8
/** Default number of approved escalations one session may take. */
export const DEFAULT_MAX_ESCALATIONS = 2

/** Plugin configuration. */
export interface Config {
  /** Weighted model-token ceiling for one budget scope; each approved escalation raises the live ceiling. Defaults to 1500000. */
  maxTokens?: number
  /** Model steps one turn may take. Defaults to 90. */
  stepsPerTurn?: number
  /**
   * What the token ceiling covers. `turn` (the default) gives every prompt its
   * own ceiling, so a session that is handed one issue after another does not
   * starve the fortieth issue of what the first one spent; `session` caps the
   * whole session.
   */
  budgetScope?: 'turn' | 'session'
  /** What one prompt-cache read token counts as against the ceiling, 0 to 1. Defaults to 0.1. */
  cacheReadWeight?: number
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
  /** Tool mask applied to committee children; omitted means every tool except {@link REVIEWER_DENIED_TOOLS}. */
  committeeTools?: { allow?: string[]; deny?: string[] }
  /**
   * Sampling temperature for every model request the session makes, when the
   * request does not already carry one. Omitted, the route's own default applies.
   */
  temperature?: number
  /**
   * Milliseconds between starting successive parts of one `run_wave` wave.
   * Defaults to {@link PART_START_STAGGER_MS}. Tests set this to 0 so a fake
   * subagent service's near-instant results still overlap without the real
   * delay; production leaves it at the default, which softens the burst a
   * shared route's rate limit otherwise sees.
   */
  partStartStaggerMs?: number
  /**
   * The most parts of one `run_wave` wave allowed to run at once. `0`
   * (default) means the whole wave runs at once, as before. A shared
   * free-tier route's account-wide rate limit can be smaller than what even
   * three or four parts ask for in a single burst — in which case every part
   * fails together no matter how they are staggered or retried, because the
   * wave asked for more in one minute than the account could ever serve. Set
   * this below a wave's typical size on such a route; the remaining parts run
   * in a later batch, after {@link interBatchDelayMs}, instead of all at once.
   */
  maxConcurrentParts?: number
  /**
   * Milliseconds to wait before starting the next batch, when
   * {@link maxConcurrentParts} splits a wave into more than one. Long enough
   * for a per-minute rate limit to partially recover; short enough not to
   * waste the turn's budget waiting. Defaults to 20 seconds. Tests set this to
   * 0.
   */
  interBatchDelayMs?: number
  /**
   * Whether the root agent's live output (text, reasoning, and tool-call
   * arguments as they stream) is mirrored to stderr for the application to
   * draw progressively. Observation only; defaults to true.
   */
  liveStream?: boolean
}

/** Runtime schema for {@link Config}. */
export const Config = z.object({
  maxTokens: z.number().step(1).min(1).default(DEFAULT_MAX_TOKENS),
  stepsPerTurn: z.number().step(1).min(1).default(DEFAULT_STEPS_PER_TURN),
  budgetScope: z.union(['turn', 'session']).default(DEFAULT_BUDGET_SCOPE),
  cacheReadWeight: z.number().min(0).max(1).default(DEFAULT_CACHE_READ_WEIGHT),
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
  temperature: z.union([z.number().min(0).max(2), z.const(undefined)]),
  partStartStaggerMs: z.number().step(1).min(0).default(PART_START_STAGGER_MS),
  maxConcurrentParts: z.number().step(1).min(0).default(0),
  interBatchDelayMs: z.number().step(1).min(0).default(DEFAULT_INTER_BATCH_DELAY_MS),
  liveStream: z.boolean().default(true),
})

/** One session's live budget state: what the ceiling currently is and what it has already done. */
interface CapState {
  /** Tokens the scope may spend, including every granted escalation. */
  limitTokens: number
  /** Escalations the user has approved for this session. */
  escalations: number
  /** The ceiling a warning was last issued for, so one ceiling warns once. */
  warnedAtLimit: number | undefined
  /** The turn the state was last updated in; a new turn opens a new turn-scoped budget. */
  turn: number | undefined
  /** Weighted spend when the current turn began, so a turn-scoped budget counts only its own. */
  turnBaseline: number
  /** Whether the current turn has already been warned about its step cap. */
  warnedSteps: boolean
  /** Steps granted past the step cap in the current turn. */
  extraSteps: number
  /** The step number `agent/pre-step` last saw, for `harness_report` to read between steps. */
  lastStep: number
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

/**
 * Why one `run_wave` part produced no usable result, in words the parent (and
 * whoever reads the transcript) can act on.
 *
 * A part that never completed carries only a `stopReason` and, sometimes, a
 * `diagnostic` — many providers give neither on a transient failure, so a bare
 * `${stopReason}` ("error") said nothing a reader could do anything with. The
 * one cause worth naming explicitly is a rate limit or shared-pool throttle,
 * because parallel parts are the harness *asking* several requests from the
 * same route at once — exactly the shape that trips a provider's burst limit
 * even when the account has quota left. This runs after one automatic retry
 * already failed (see the retry loop in `execute`), so it also says that.
 * @param result - the part's terminal result, when the run settled at all.
 * @param thrown - the error that replaced a result, when starting or awaiting it threw.
 * @returns one sentence for the part's line in the wave's report.
 */
function partFailure(result: SubagentResult | undefined, thrown: string | undefined): string {
  if (thrown !== undefined) return `the part's agent could not be started (retried once): ${thrown}`
  if (result === undefined) return 'the part settled without a result (retried once)'
  if (result.stopReason !== 'completed') {
    const detail = result.diagnostic ?? 'the provider gave no further detail'
    return `the model request did not complete after a retry (${result.stopReason}: ${detail}).`
      + ' Parts run in parallel share one route\'s quota, so this is often the route\'s own rate limit or a'
      + ' concurrent-request throttle rather than anything about this part\'s task — rerunning the whole job, or'
      + ' asking for this one part again, often succeeds once the burst has passed.'
  }
  return 'the part completed without reporting done or failed through structured_output (retried once).'
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
  const budgetScope = config.budgetScope ?? DEFAULT_BUDGET_SCOPE
  const cacheReadWeight = config.cacheReadWeight ?? DEFAULT_CACHE_READ_WEIGHT
  const warnAt = config.warnAt ?? DEFAULT_WARN_AT
  const grantTokens = config.grantTokens ?? Math.floor(maxTokens / 2)
  const maxEscalations = config.maxEscalations ?? DEFAULT_MAX_ESCALATIONS
  const provider = config.committeeProvider ?? 'spawn'
  const bench = resolveRoles(config.committeeRoles ?? REVIEW_ROLES.map(role => role.id))
  const tools = config.committeeTools
  const caps = new WeakMap<Session, CapState>()

  /**
   * The default reviewer mask: every tool but the ones that change the
   * workspace or seat more reviewers. The persona already says "never modify
   * the workspace"; the mask is what makes that true for a model that forgets.
   */
  const reviewerFilter = (agent: Parameters<typeof ctx.tools.get>[1]): { deny: string[] } => ({
    deny: REVIEWER_DENIED_TOOLS.filter(name => ctx.tools.get(name, agent) !== undefined),
  })

  /** Files each live `run_wave` child may write, keyed by the child's session id. */
  const owners = new Map<string, readonly string[]>()
  /**
   * Every `run_wave` part outcome this session has recorded, keyed by the
   * parent session and capped so a long-lived session cannot grow this
   * without bound. `harness_report` reads it; nothing else does.
   */
  const waveLog = new Map<Session, WaveOutcomeLog[]>()
  /** Part outcomes kept per session before the oldest are dropped. */
  const WAVE_LOG_LIMIT = 50

  // A part's child writes only the files its part declared. The plan is
  // file-disjoint only if that holds, and a prompt alone does not make it hold:
  // this is the harness enforcing it, one lookup per write.
  ctx.on('tools/pre-execute', async (exec, next) => {
    const argument = WRITE_TOOLS[exec.name]
    const session = exec.agent?.session
    const owned = session === undefined ? undefined : owners.get(String(session.id))
    if (argument === undefined || owned === undefined || session === undefined) return next()
    const target = (exec.arguments as Record<string, unknown> | null)?.[argument]
    if (typeof target === 'string' && !ownsPath(owned, target, session.header.cwd ?? process.cwd())) {
      return {
        kind: 'deny',
        reason: `${RUN_WAVE}: this part owns ${owned.length === 0 ? 'no file' : owned.join(', ')}; ${target} is`
          + ' another part\'s or nobody\'s. Do not write it — say in your summary what it needs.',
      }
    }
    return next()
  })

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
        toolFilter: tools ?? reviewerFilter(agent),
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

  ctx.tools.register(defineTool({
    name: RUN_WAVE,
    description: RUN_WAVE_DESCRIPTION,
    parameters: {
      parts: {
        type: 'array',
        required: true,
        description: 'Every part of the job. Parts that share no file run at the same time; overlapping ones run in later waves.',
        items: {
          type: 'object',
          additionalProperties: false,
          properties: {
            id: { type: 'string', required: true, description: 'Short stable name for this part, unique in the list.' },
            task: { type: 'string', required: true, description: 'What the part must do, complete enough for an agent that has not seen this conversation.' },
            files: {
              type: 'array',
              required: true,
              items: { type: 'string' },
              description: 'Workspace-relative files this part creates or edits; its agent may write only these.',
            },
            check: { type: 'string', description: 'The command that proves the part (a test or a run).' },
          },
        },
      },
    },
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          waves: { type: 'number', required: true },
          parts: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                id: { type: 'string', required: true },
                wave: { type: 'number', required: true },
                status: { type: 'string', required: true },
                summary: { type: 'string', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => [{ type: 'text', text: renderRun(value.parts as PartOutcome[], value.waves) }],
    },
    execute: async (args, exec) => {
      const agent = exec.agent
      if (agent === undefined) throw new Error(`${RUN_WAVE} requires a calling agent`)
      const subagents = ctx.get('subagents')
      if (subagents === undefined) {
        throw new Error(`${RUN_WAVE}: no subagent service is composed; build the parts yourself, one at a time`)
      }
      const parts: WavePart[] = args.parts.map(part => ({
        id: part.id.trim(),
        task: part.task,
        files: part.files,
        ...part.check === undefined ? {} : { check: part.check },
      }))
      const plan = planWaves(parts)
      const byId = new Map(parts.map(part => [part.id, part]))
      // The frozen spec, when the spec lock is composed: children start with no
      // transcript, and the job is the context their part sits in.
      const specState = (ctx.get('sessionProjections') as { stateOf?: (session: Session, key: string) => unknown } | undefined)
        ?.stateOf?.(agent.session, 'specLock') as { text?: string | null } | undefined
      const spec = specState?.text ?? undefined
      const outcomes: PartOutcome[] = []
      let wavesRun = 0
      for (const wave of plan.waves) {
        if (exec.signal.aborted) {
          outcomes.push(...wave.subtasks.map(id => ({ id, wave: wave.index, status: 'failed' as const, summary: 'Cancelled before this part started.' })))
          continue
        }
        wavesRun += 1
        const members = wave.subtasks.flatMap(id => byId.get(id) ?? [])
        const batches = batchParts(members, maxConcurrentParts)
        for (const [batchIndex, batch] of batches.entries()) {
          if (batchIndex > 0 && !exec.signal.aborted) await sleep(interBatchDelay)
          const settled = await Promise.all(batch.map(async (part, index): Promise<PartOutcome> => {
            const siblings = members.filter(other => other.id !== part.id).map(other => other.id)
            // Every part in a wave shares one route's quota. Starting all of them
            // in the exact same instant is the shape most likely to trip a
            // provider's per-second or concurrent-request throttle — a stagger
            // keeps this "parallel" (every part still runs at once, seconds
            // apart, not one after another's full turn) while landing far softer
            // on that limit. Retries below already carry their own delay, so the
            // stagger applies to the first attempt only.
            if (index > 0) await sleep(index * partStagger)
            if (exec.signal.aborted) {
              return { id: part.id, wave: wave.index, status: 'failed', summary: 'Cancelled before this part started.' }
            }
            let lastResult: SubagentResult | undefined
            let lastThrown: string | undefined
            for (let attempt = 1; attempt <= 1 + PART_RETRIES; attempt += 1) {
              let run: SubagentRun | undefined
              try {
                run = await subagents.start(provider, {
                  label: `${RUN_WAVE}:${part.id}`,
                  prompt: [{ type: 'text', text: partPrompt(part, spec, siblings) }],
                  parent: agent,
                  signal: exec.signal,
                  outputSchema: PART_RESULT_SCHEMA as unknown as ObjectJsonSchema,
                  maxDepth: 1,
                  toolFilter: { deny: PART_DENIED_TOOLS.filter(name => ctx.tools.get(name, agent) !== undefined) },
                })
                owners.set(String(run.id), part.files)
                const result = await run.result
                if (result.stopReason === 'completed') {
                  const structured = result.structured as { status?: unknown; summary?: unknown } | undefined
                  const summary = typeof structured?.summary === 'string' && structured.summary.trim().length > 0
                    ? structured.summary.trim()
                    : partFailure(result, undefined)
                  return {
                    id: part.id,
                    wave: wave.index,
                    status: structured?.status === 'done' ? 'done' : 'failed',
                    summary,
                  }
                }
                // Not `completed`: an infrastructure- or provider-level failure
                // (a thrown transport error settles the same way, in the model),
                // which is exactly the shape a transient rate limit or throttle
                // takes — worth one retry before this part is reported failed.
                lastResult = result
              } catch (error: unknown) {
                lastThrown = describeError(error)
              } finally {
                if (run !== undefined) {
                  await run.dispose().catch((error: unknown) => {
                    ctx.logger.warn(`harness-preset: ${RUN_WAVE}: releasing part ${part.id} failed: ${describeError(error)}`)
                  })
                  owners.delete(String(run.id))
                }
              }
              if (attempt <= PART_RETRIES && !exec.signal.aborted) await sleep(PART_RETRY_DELAY_MS)
            }
            return { id: part.id, wave: wave.index, status: 'failed', summary: partFailure(lastResult, lastThrown) }
          }))
          outcomes.push(...settled)
        }
      }
      const log = waveLog.get(agent.session) ?? []
      log.push(...outcomes.map(outcome => ({ id: outcome.id, wave: outcome.wave, status: outcome.status, summary: outcome.summary })))
      waveLog.set(agent.session, log.length > WAVE_LOG_LIMIT ? log.slice(log.length - WAVE_LOG_LIMIT) : log)
      return { waves: wavesRun, parts: outcomes }
    },
    presentCall: () => ({ card: 'generic', title: 'Run parts as parallel subagents', kind: 'edit' }),
    presentResult: () => ({ card: 'generic', title: 'Parts built' }),
  }))

  ctx.tools.register(defineTool({
    name: HARNESS_REPORT,
    description: 'Self-check this turn: weighted token spend, step count, and escalations against their live ceilings,'
      + ' plus the outcome of every run_wave part run so far, turned into a short findings report. Read-only — call it'
      + ' before reporting a multi-part job done, or any time you want evidence the turn is still within bounds.',
    parameters: {},
    output: {
      schema: {
        type: 'object',
        additionalProperties: false,
        properties: {
          budget: {
            type: 'object',
            required: true,
            additionalProperties: false,
            properties: {
              tokensSpent: { type: 'number', required: true },
              tokensLimit: { type: 'number', required: true },
              stepsUsed: { type: 'number', required: true },
              stepsLimit: { type: 'number', required: true },
              escalations: { type: 'number', required: true },
              maxEscalations: { type: 'number', required: true },
            },
          },
          waveRuns: {
            type: 'object',
            required: true,
            additionalProperties: false,
            properties: {
              total: { type: 'number', required: true },
              failed: { type: 'number', required: true },
            },
          },
          findings: {
            type: 'array',
            required: true,
            items: {
              type: 'object',
              additionalProperties: false,
              properties: {
                severity: { type: 'string', required: true },
                summary: { type: 'string', required: true },
                detail: { type: 'string', required: true },
              },
            },
          },
        },
      },
      render: (_args, value) => [{ type: 'text', text: renderReport(value as TurnReport) }],
    },
    execute: async (_args, exec) => {
      const agent = exec.agent
      if (agent === undefined) throw new Error(`${HARNESS_REPORT} requires a calling agent`)
      const session = agent.session
      const cap = caps.get(session)
      const projections = ctx.get('sessionProjections')
      const usage = projections?.stateOf(session, 'tokenUsage')
      const spentRaw = usage === undefined ? 0 : weightedTokens(usage.totals, cacheReadWeight)
      const tokensSpent = cap === undefined
        ? 0
        : budgetScope === 'turn' ? Math.max(0, spentRaw - cap.turnBaseline) : spentRaw
      const budget: BudgetSnapshot = {
        tokensSpent,
        tokensLimit: cap?.limitTokens ?? maxTokens,
        stepsUsed: cap?.lastStep ?? 0,
        stepsLimit: stepsPerTurn + (cap?.extraSteps ?? 0),
        escalations: cap?.escalations ?? 0,
        maxEscalations,
      }
      return buildReport(budget, waveLog.get(session) ?? [])
    },
    presentCall: () => ({ card: 'generic', title: 'Harness self-check', kind: 'read' }),
    presentResult: () => ({ card: 'generic', title: 'Self-check report' }),
  }))

  // Live output for the application. The ACP bridge only forwards *committed*
  // messages, so a streamed answer (and a tool call whose arguments carry a
  // whole file) reached the window in one piece at the end of each step — the
  // turn looked stuck, then flashed. This listener only observes the root
  // agent's live stream and mirrors each delta to stderr as one tagged JSON
  // line, which the application reads beside the ACP channel and draws as a
  // provisional bubble until the committed message replaces it. It never
  // touches a request, a tool, the transcript, or the budget.
  if (config.liveStream ?? true) {
    ctx.on('agent/assistant-stream', ({ agent, frame }) => {
      if (agent.session.header.parentSession !== undefined) return
      const base = { session: String(agent.session.id), attempt: String(frame.attemptId) }
      let line: Record<string, unknown> | undefined
      if (frame.type === 'start') line = { ...base, kind: 'start' }
      else if (frame.type === 'end') line = { ...base, kind: 'end' }
      else {
        const chunk = frame.chunk
        if (chunk.type === 'text-delta' && chunk.text !== '') line = { ...base, kind: 'text', text: chunk.text }
        else if (chunk.type === 'reasoning-delta' && chunk.text !== '') line = { ...base, kind: 'thought', text: chunk.text }
        else if (chunk.type === 'tool-call-delta') {
          line = { ...base, kind: 'tool', id: String(chunk.id), name: chunk.name, text: chunk.argumentsDelta }
        }
      }
      if (line !== undefined) process.stderr.write(`${LIVE_STREAM_TAG}${JSON.stringify(line)}\n`)
    })
  }

  // One temperature for the whole session, set on the request header rather than
  // per call: the header is what the prompt cache keys on, so a constant value
  // costs nothing, while a value that moved between steps would miss the cache.
  const partStagger = config.partStartStaggerMs ?? PART_START_STAGGER_MS
  const maxConcurrentParts = config.maxConcurrentParts ?? 0
  const interBatchDelay = config.interBatchDelayMs ?? DEFAULT_INTER_BATCH_DELAY_MS
  const temperature = config.temperature
  if (temperature !== undefined) {
    ctx.on('agent/request', async (_payload, next) => {
      const call = await next()
      return call.temperature === undefined ? { ...call, temperature } : call
    })
  }

  ctx.on('agent/pre-step', async ({ agent, turn, step, signal }, next): Promise<PreStepDecision> => {
    const decision = await next()
    if (decision.kind === 'reject' || signal.aborted) return decision
    const session = agent.session
    const projections = ctx.get('sessionProjections')
    const usage = projections?.stateOf(session, 'tokenUsage')
    // A composition without the token meter cannot measure spend, so only the
    // step cap can fire there; the ceiling still governs escalation.
    const spent = usage === undefined ? 0 : weightedTokens(usage.totals, cacheReadWeight)
    let cap = caps.get(session)
    if (cap === undefined) {
      cap = {
        limitTokens: maxTokens,
        escalations: 0,
        warnedAtLimit: undefined,
        turn,
        turnBaseline: spent,
        warnedSteps: false,
        extraSteps: 0,
        lastStep: step,
      }
      caps.set(session, cap)
    } else if (cap.turn !== turn) {
      // A new prompt is a new job. Its step warning is its own, and under a
      // turn-scoped budget so is its ceiling: grants bought for the last issue
      // do not carry over, and neither does the last issue's spend.
      cap.turn = turn
      cap.warnedSteps = false
      cap.extraSteps = 0
      if (budgetScope === 'turn') {
        cap.turnBaseline = spent
        cap.limitTokens = maxTokens
        cap.warnedAtLimit = undefined
      }
    }
    cap.lastStep = step
    const verdict = decideBudget(
      { tokens: cap.limitTokens, stepsPerTurn: stepsPerTurn + cap.extraSteps, warnAt, scope: budgetScope },
      {
        tokens: budgetScope === 'turn' ? spent - cap.turnBaseline : spent,
        stepsInTurn: step,
      },
    )
    if (verdict.kind === 'ok') return decision
    if (verdict.kind === 'warn') {
      if (verdict.over === 'steps') {
        if (cap.warnedSteps) return decision
        cap.warnedSteps = true
      } else {
        if (cap.warnedAtLimit === cap.limitTokens) return decision
        cap.warnedAtLimit = cap.limitTokens
      }
      return { ...decision, messages: [...decision.messages, budgetMessage(verdict.text)] }
    }
    const approval = ctx.get('approval')
    const root = session.header.parentSession === undefined
    if (approval !== undefined && root && cap.escalations < maxEscalations) {
      // A grant buys more of whatever ran out: a token grant cannot help a turn
      // stopped by its step cap, and granting one there bought a single step.
      const grantSteps = Math.max(1, Math.ceil(stepsPerTurn / 2))
      const grant = verdict.over === 'steps'
        ? `${formatCount(grantSteps)} more steps`
        : `${formatCount(grantTokens)} more model tokens`
      const outcome = await approval.request({
        agent,
        toolName: BUDGET_TOOL,
        callId: brandString<ToolCallId>(BUDGET_TOOL),
        reason: `${verdict.text} Grant ${grant}`
          + ` (escalation ${formatCount(cap.escalations + 1)} of ${formatCount(maxEscalations)})?`,
        signal,
      })
      if (outcome === 'allowed-once') {
        cap.escalations += 1
        if (verdict.over === 'steps') {
          cap.extraSteps += grantSteps
          cap.warnedSteps = false
        } else {
          cap.limitTokens += grantTokens
        }
        ctx.logger.info(`harness-preset: user granted ${grant}; the ceiling is now`
          + ` ${String(cap.limitTokens)} tokens and ${String(stepsPerTurn + cap.extraSteps)} steps`)
        return decision
      }
      ctx.logger.info(`harness-preset: budget escalation ${outcome}; the turn ends here`)
    }
    ctx.logger.warn(`harness-preset: ${verdict.text}`)
    return { kind: 'reject' }
  })
}
