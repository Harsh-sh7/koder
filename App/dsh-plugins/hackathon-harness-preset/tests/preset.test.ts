/**
 * The preset is only worth its tokens if a session really reads what it
 * contributes, so these tests compose the plugin against the repository's own
 * prompt and tool services and then drive the step seam the way the loop does:
 * one claimed user message, a step number, and the decision the next listener
 * would have produced. They pin the three surfaces that carry the contract —
 * the constant prompt section, the two tool schemas, and the shipped skill
 * body — plus the one behaviour no seam other than `agent/pre-step` can enforce:
 * the budget warns once per ceiling and stops the turn when it is spent.
 *
 * The budget's own arithmetic is pinned in `budget.test.ts`; here the spend is a
 * fake projection and the approval service is a fake answerer, because what is
 * under test is the preset's wiring, not the token meter or the approval
 * channel.
 */

import { strict as assert } from 'node:assert'
import { test } from 'node:test'

import { Context } from '@deepseek-ai/cordis'
import type { Agent, PreStepDecision } from '@deepseek-ai/dsh-agent'
import { createUserMessage } from '@deepseek-ai/dsh-llm'
import type { UserMessage } from '@deepseek-ai/dsh-llm'
import SessionStore, { SessionId } from '@deepseek-ai/dsh-session'
import SystemPrompt from '@deepseek-ai/dsh-system-prompt'
import ToolRuntime from '@deepseek-ai/dsh-tools'

import * as harnessPreset from '../src/index.ts'
import { BUDGET_TOOL, COMMITTEE, CONTRACT_SECTION, ORCHESTRATION_SKILL, WAVE_PLAN, contractText } from '../src/index.ts'
import type { Config } from '../src/index.ts'

/** The mutable spend the fake token-usage projection serves, bucket by bucket. */
interface Spend {
  uncachedInputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheWriteTokens: number
}

/** One registered skill definition, as the plugin handed it to the skills seam. */
interface RegisteredSkill {
  name: string
  description: string
  content: string
}

/** A composed harness plus the seams the preset reads and the test observes. */
async function harness(config: Config = {}, approval?: 'allowed-once' | 'rejected') {
  const ctx = new Context()
  await ctx.plugin(SessionStore)
  await ctx.plugin(SystemPrompt)
  await ctx.plugin(ToolRuntime)

  const spend: Spend = { uncachedInputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 }
  ctx.provide('sessionProjections', { stateOf: () => ({ totals: spend }) } as never)
  const skills: RegisteredSkill[] = []
  ctx.provide('skills', {
    register: (definition: RegisteredSkill) => {
      skills.push(definition)
      return () => {}
    },
  } as never)
  const asked: { toolName: string; reason?: string }[] = []
  if (approval !== undefined) {
    ctx.provide('approval', {
      request: async (request: { toolName: string; reason?: string }) => {
        asked.push(request)
        return approval
      },
    } as never)
  }

  await ctx.plugin(harnessPreset, config)

  const session = ctx.sessions.create(SessionId('harness'))
  const agent = { session } as unknown as Agent
  const claimed = (): UserMessage[] => [createUserMessage({ content: [{ type: 'text', text: 'build the thing' }], source: { kind: 'user' } })]
  /** One proposed step, driven through the waterfall exactly as the loop drives it. */
  const step = (stepNumber = 1, messages: UserMessage[] = claimed()): Promise<PreStepDecision> =>
    ctx.waterfall(
      'agent/pre-step',
      { agent, messages, turn: 1, step: stepNumber, signal: new AbortController().signal },
      () => Promise.resolve<PreStepDecision>({ kind: 'enter', messages }),
    )
  return { ctx, spend, skills, asked, step }
}

/** The messages of an admitting decision, failing the test when the harness rejected the step. */
function admitted(decision: PreStepDecision): readonly UserMessage[] {
  if (decision.kind === 'reject') assert.fail('the harness rejected the step')
  return decision.messages
}

/** A decision's messages as plain text, so an assertion reads like the model's view. */
function texts(messages: readonly UserMessage[]): string[] {
  return messages.map(message => message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join(''))
}

test('the operating contract rides every request as constant text', async () => {
  const { ctx } = await harness()
  const assembly = await ctx.systemPrompt.assemble()
  const section = assembly.sections.find(candidate => candidate.name === CONTRACT_SECTION)
  assert.ok(section !== undefined, 'the contract section must be registered')
  assert.equal(section.text, contractText())
  // The prefix is cacheable only while it stays byte-identical between steps,
  // which is why no live number (spend, ceiling, step) may reach it.
  const next = await ctx.systemPrompt.assemble()
  assert.equal(next.sections.find(candidate => candidate.name === CONTRACT_SECTION)?.text, section.text)
})

test('the two tools declare the parameters the contract depends on', async () => {
  const { ctx } = await harness()
  const byName = new Map(ctx.tools.schemas().map(schema => [schema.name, schema]))

  const wave = byName.get(WAVE_PLAN)
  assert.ok(wave !== undefined, 'wave_plan must be callable in a session')
  assert.match(wave.description, /safe to run in parallel/u)
  assert.deepEqual((wave.parameters as { required?: unknown }).required, ['subtasks'])

  const committee = byName.get(COMMITTEE)
  assert.ok(committee !== undefined, 'committee must be callable in a session')
  assert.match(committee.description, /quorum/u)
  // A decision is the one thing the review cannot default: everything else has
  // a declared default, so only these may be required by the schema.
  assert.deepEqual((committee.parameters as { required?: unknown }).required, ['decision'])
})

test('the orchestration skill ships with the plugin and loads on demand', async () => {
  const { skills } = await harness()
  assert.deepEqual(skills.map(skill => skill.name), [ORCHESTRATION_SKILL])
  const skill = skills[0]
  assert.ok(skill !== undefined)
  // The catalogue line is what the router pays for every step; the body is what
  // a loaded skill pays for once. Both have to name the workflow they hand over.
  assert.match(skill.description, /file-disjoint waves/u)
  assert.match(skill.content, /wave_plan/u)
  assert.match(skill.content, /committee/u)
})

test('the budget warns once per ceiling, not once per step', async () => {
  const { spend, step } = await harness({ maxTokens: 1_000, warnAt: 0.8, stepsPerTurn: 50 })
  spend.uncachedInputTokens = 800
  const warned = admitted(await step(2))
  assert.deepEqual(texts(warned).slice(-1), [
    'Harness budget: 80% spent (800 of 1,000 model tokens). Finish the subtask in progress and report; do not start new work.',
  ])
  assert.equal(warned.at(-1)?.source.kind, 'harness-budget')
  // The same ceiling has already been warned for: the next step must not pay
  // for the same sentence again.
  assert.equal(admitted(await step(3)).length, 1)
})

test('an exhausted budget stops the turn when nobody can be asked', async () => {
  const { spend, asked, step } = await harness({ maxTokens: 1_000 })
  spend.outputTokens = 1_000
  assert.deepEqual(await step(2), { kind: 'reject' })
  assert.deepEqual(asked, [])
})

test('an approved escalation raises the ceiling and admits the step', async () => {
  const { spend, asked, step } = await harness({ maxTokens: 1_000, grantTokens: 500, maxEscalations: 2 }, 'allowed-once')
  spend.outputTokens = 1_000
  const granted = admitted(await step(2))
  assert.equal(granted.length, 1)
  assert.deepEqual(asked.map(request => request.toolName), [BUDGET_TOOL])
  assert.match(asked[0]?.reason ?? '', /Grant 500 more model tokens \(escalation 1 of 2\)\?/u)
  // The grant is real: the same spend now sits under the raised ceiling, so the
  // next step is admitted without a warning.
  assert.equal(admitted(await step(3)).length, 1)
})
