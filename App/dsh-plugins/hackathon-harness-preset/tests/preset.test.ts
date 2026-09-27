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
import { BUDGET_TOOL, COMMITTEE, CONTRACT_SECTION, HARNESS_REPORT, ORCHESTRATION_SKILL, RUN_WAVE, WAVE_PLAN, contractText } from '../src/index.ts'
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
  const step = (stepNumber = 1, messages: UserMessage[] = claimed(), turn = 1): Promise<PreStepDecision> =>
    ctx.waterfall(
      'agent/pre-step',
      { agent, messages, turn, step: stepNumber, signal: new AbortController().signal },
      () => Promise.resolve<PreStepDecision>({ kind: 'enter', messages }),
    )
  return { ctx, spend, skills, asked, step, session, agent }
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

test('delegation is an unconditional rule the model reaches before writing any file, not a judgement call', async () => {
  // Regression pin: a request like "a todo CLI with tests and a README" reads
  // as one small task to a weak model, so a rule buried near the end of the
  // contract ("...when a job has two or more parts...") was reliably skipped —
  // the model just wrote every file itself, serially, no run_wave, no
  // subagents. The fix is textual and has to keep two properties: the rule
  // rides EARLY (models weight earlier instructions more) and its trigger is
  // CONCRETE ("an implementation plus its tests is already two parts") rather
  // than a judgement call the model has to first decide applies to it.
  const text = contractText()
  const lines = text.split('\n')
  const delegate = lines.findIndex(line => /\brun_wave\b/u.test(line) && /\bcall\b/iu.test(line))
  assert.ok(delegate !== -1, 'a rule must instruct calling run_wave')
  // Earlier than halfway through the numbered rules: this is what makes it
  // reliably followed rather than merely present somewhere in the prompt.
  assert.ok(delegate <= lines.length / 2, `run_wave rule at line ${delegate} of ${lines.length} is not early enough`)
  assert.match(text, /implementation.{0,40}tests.{0,80}already (two|2) parts/iu)
  assert.match(text, /however small or simple the task reads/u)
  assert.match(text, /do not (write|ask)/u)
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
  const { spend, step } = await harness({ maxTokens: 1_000, warnAt: 0.8, stepsPerTurn: 50, budgetScope: 'session' })
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
  const { spend, asked, step } = await harness({ maxTokens: 1_000, budgetScope: 'session' })
  spend.outputTokens = 1_000
  assert.deepEqual(await step(2), { kind: 'reject' })
  assert.deepEqual(asked, [])
})

test('an approved escalation raises the ceiling and admits the step', async () => {
  const { spend, asked, step } = await harness(
    { maxTokens: 1_000, grantTokens: 500, maxEscalations: 2, budgetScope: 'session' },
    'allowed-once',
  )
  spend.outputTokens = 1_000
  const granted = admitted(await step(2))
  assert.equal(granted.length, 1)
  assert.deepEqual(asked.map(request => request.toolName), [BUDGET_TOOL])
  assert.match(asked[0]?.reason ?? '', /Grant 500 more model tokens \(escalation 1 of 2\)\?/u)
  // The grant is real: the same spend now sits under the raised ceiling, so the
  // next step is admitted without a warning.
  assert.equal(admitted(await step(3)).length, 1)
})

test('a turn-scoped budget gives every prompt its own ceiling', async () => {
  const { spend, step } = await harness({ maxTokens: 1_000, stepsPerTurn: 50 })
  // The first issue spends almost everything and is still admitted.
  assert.equal(admitted(await step(1, undefined, 1)).length, 1)
  spend.outputTokens = 990
  assert.equal(admitted(await step(2, undefined, 1)).length, 2, 'the first turn is warned at its own 80%')
  // The next prompt starts from zero: the last issue's spend is not its debt.
  assert.equal(admitted(await step(1, undefined, 2)).length, 1)
  spend.outputTokens = 990 + 700
  assert.equal(admitted(await step(2, undefined, 2)).length, 1)
  spend.outputTokens = 990 + 1_000
  assert.deepEqual(await step(3, undefined, 2), { kind: 'reject' })
})

test('prompt-cache reads count at their weight against the ceiling', async () => {
  const { spend, step } = await harness({ maxTokens: 1_000, stepsPerTurn: 50, budgetScope: 'session' })
  // Nine thousand cached tokens at the default weight of 0.1 are 900: warned, not stopped.
  spend.cacheReadTokens = 9_000
  const warned = admitted(await step(2))
  assert.match(texts(warned).at(-1) ?? '', /Harness budget: 90% spent/u)
})

test('the step cap warns before it stops, and a grant on it buys steps, not tokens', async () => {
  const { asked, step } = await harness({ stepsPerTurn: 4, warnAt: 0.5, maxEscalations: 1 }, 'allowed-once')
  assert.match(texts(admitted(await step(2))).at(-1) ?? '', /step 2 of a 4-step turn\. Converge now/u)
  // Warned once per turn.
  assert.equal(admitted(await step(3)).length, 1)
  // Step 5 is over the cap: the grant adds half the cap again, so steps 5 and 6 run.
  assert.ok(admitted(await step(5)).length >= 1)
  assert.match(asked[0]?.reason ?? '', /Grant 2 more steps \(escalation 1 of 1\)\?/u)
  assert.ok(admitted(await step(6)).length >= 1)
  // The only escalation is spent, so the next overrun ends the turn.
  assert.deepEqual(await step(7), { kind: 'reject' })
})

test('a configured temperature reaches the request header unless the request set its own', async () => {
  const { ctx } = await harness({ temperature: 0.2 })
  const request = (seed: { provider: string; model: string; temperature?: number }) => ctx.waterfall(
    'agent/request',
    { agent: {} as Agent, turn: 1, step: 1, signal: new AbortController().signal },
    () => Promise.resolve(seed),
  )
  assert.equal((await request({ provider: 'p', model: 'm' })).temperature, 0.2)
  assert.equal((await request({ provider: 'p', model: 'm', temperature: 0.9 })).temperature, 0.9)
})

test('run_wave seats every part of a wave at once, runs overlapping parts later, and returns each result', async () => {
  // The real stagger between starts is a deliberate, small wall-clock delay
  // (softening a burst against a rate-limited route); zeroing it here keeps
  // this test fast and deterministic without weakening what it proves.
  const { ctx } = await harness({ partStartStaggerMs: 0 })
  const started: { label: string; wave: number }[] = []
  let wave = 1
  let live = 0
  let peak = 0
  ctx.provide('subagents', {
    start: async (_provider: string, request: { label: string; prompt: { text: string }[] }) => {
      started.push({ label: request.label, wave })
      live += 1
      peak = Math.max(peak, live)
      const id = `child-${request.label}`
      return {
        id,
        localAgent: undefined,
        result: (async () => {
          await new Promise(resolve => setTimeout(resolve, 5))
          live -= 1
          if (live === 0) wave += 1
          return request.label.endsWith(':docs')
            ? { output: [], stopReason: 'completed', structured: { status: 'failed', summary: 'blocked on the name.' } }
            : { output: [], stopReason: 'completed', structured: { status: 'done', summary: `${request.label} ok.` } }
        })(),
        dispose: async () => {},
      }
    },
  } as never)
  const tool = ctx.tools.get(RUN_WAVE)
  assert.ok(tool !== undefined, 'run_wave must be callable in a session')
  const session = ctx.sessions.create(SessionId('waves'))
  const value = await tool.execute({
    parts: [
      { id: 'core', task: 'Write the CLI.', files: ['todo.py'], check: 'python todo.py list' },
      { id: 'tests', task: 'Write tests.', files: ['test_todo.py'], check: 'pytest -q' },
      { id: 'docs', task: 'Write the README.', files: ['README.md'] },
      { id: 'polish', task: 'Add --help text.', files: ['todo.py'] },
    ],
  }, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never) as {
    waves: number
    parts: { id: string; wave: number; status: string }[]
  }
  // Three disjoint parts share wave 1 and ran together; the part that shares
  // todo.py waited for wave 2.
  assert.equal(value.waves, 2)
  assert.equal(peak, 3)
  assert.deepEqual(started.map(entry => [entry.label, entry.wave]), [
    ['run_wave:core', 1], ['run_wave:tests', 1], ['run_wave:docs', 1], ['run_wave:polish', 2],
  ])
  assert.deepEqual(value.parts.map(part => [part.id, part.wave, part.status]), [
    ['core', 1, 'done'], ['tests', 1, 'done'], ['docs', 1, 'failed'], ['polish', 2, 'done'],
  ])
})

test('a run_wave child may write its own files and is refused any other', async () => {
  const { ctx } = await harness()
  let release: (() => void) | undefined
  const gate = new Promise<void>((resolve) => { release = resolve })
  ctx.provide('subagents', {
    start: async () => ({
      id: 'child-core',
      localAgent: undefined,
      result: gate.then(() => ({ output: [], stopReason: 'completed', structured: { status: 'done', summary: 'ok' } })),
      dispose: async () => {},
    }),
  } as never)
  const tool = ctx.tools.get(RUN_WAVE)
  assert.ok(tool !== undefined)
  const parent = ctx.sessions.create(SessionId('parent'))
  const running = tool.execute(
    { parts: [{ id: 'core', task: 'Write the CLI.', files: ['src/todo.py'] }] },
    { agent: { session: parent } as unknown as Agent, signal: new AbortController().signal } as never,
  )
  await new Promise(resolve => setTimeout(resolve, 5))
  const child = { session: { id: 'child-core', header: { cwd: '/work' } } } as unknown as Agent
  const decide = (file: string) => ctx.waterfall(
    'tools/pre-execute',
    { name: 'write', arguments: { file_path: file, content: '' }, agent: child } as never,
    () => Promise.resolve({ kind: 'allow' as const }),
  )
  assert.deepEqual(await decide('src/todo.py'), { kind: 'allow' })
  const refused = await decide('README.md') as { kind: string; reason?: string }
  assert.equal(refused.kind, 'deny')
  assert.match(refused.reason ?? '', /owns src\/todo\.py; README\.md is another part's/u)
  release?.()
  await running
  // Once the part settles, its child's writes are no longer the harness's business.
  assert.deepEqual(await decide('README.md'), { kind: 'allow' })
})


test('cancelled waves report every skipped part without starting children', async () => {
  const { ctx } = await harness()
  ctx.provide('subagents', { start: async () => { throw new Error('must not start') } } as never)
  const tool = ctx.tools.get(RUN_WAVE)!
  const session = ctx.sessions.create(SessionId('cancelled-waves'))
  const abort = new AbortController()
  abort.abort()
  const value = await tool.execute({ parts: [
    { id: 'a', task: 'First', files: ['same'] },
    { id: 'b', task: 'Second', files: ['same'] },
  ] }, { agent: { session } as unknown as Agent, signal: abort.signal } as never) as {
    waves: number; parts: { status: string; summary: string }[]
  }
  assert.equal(value.waves, 0)
  assert.equal(value.parts.length, 2)
  for (const part of value.parts) {
    assert.equal(part.status, 'failed')
    assert.match(part.summary, /Cancelled before/u)
  }
})

test('a part whose model request fails without completing is retried once before being reported failed', async () => {
  // This is the real bug behind three simultaneous parts all reporting the
  // bare word "error": a route that is shared across a wave's parallel starts
  // can throttle one of them transiently — the part's task was never the
  // problem, the burst against the route was. One retry, and a message that
  // says so, is the fix; a part that completed and genuinely reported failed
  // through structured_output must NOT be retried (that is a real verdict).
  const { ctx } = await harness({ partStartStaggerMs: 0 })
  const attempts = new Map<string, number>()
  ctx.provide('subagents', {
    start: async (_provider: string, request: { label: string }) => {
      const part = request.label.split(':')[1] ?? request.label
      const attempt = (attempts.get(part) ?? 0) + 1
      attempts.set(part, attempt)
      if (part === 'flaky' && attempt === 1) {
        return {
          id: `child-${part}-${attempt}`,
          localAgent: undefined,
          result: Promise.resolve({ output: [], stopReason: 'error' }),
          dispose: async () => {},
        }
      }
      if (part === 'wrong') {
        return {
          id: `child-${part}-${attempt}`,
          localAgent: undefined,
          result: Promise.resolve({
            output: [],
            stopReason: 'completed',
            structured: { status: 'failed', summary: 'the file the task named does not exist.' },
          }),
          dispose: async () => {},
        }
      }
      return {
        id: `child-${part}-${attempt}`,
        localAgent: undefined,
        result: Promise.resolve({
          output: [],
          stopReason: 'completed',
          structured: { status: 'done', summary: `${part} ok on attempt ${attempt}.` },
        }),
        dispose: async () => {},
      }
    },
  } as never)
  const tool = ctx.tools.get(RUN_WAVE)
  assert.ok(tool !== undefined)
  const session = ctx.sessions.create(SessionId('retry'))
  const value = await tool.execute({
    parts: [
      { id: 'flaky', task: 'Write the core module.', files: ['core.py'] },
      { id: 'wrong', task: 'Write a file that cannot exist.', files: ['nope.py'] },
    ],
  }, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never) as {
    parts: { id: string; status: string; summary: string }[]
  }
  assert.equal(attempts.get('flaky'), 2, 'the transient failure earns exactly one retry')
  assert.equal(attempts.get('wrong'), 1, 'a real reported failure is never retried')
  const flaky = value.parts.find(part => part.id === 'flaky')
  assert.equal(flaky?.status, 'done')
  assert.match(flaky?.summary ?? '', /attempt 2/u)
  const wrong = value.parts.find(part => part.id === 'wrong')
  assert.equal(wrong?.status, 'failed')
  assert.match(wrong?.summary ?? '', /does not exist/u)
})

test('a part still failing after its retry names the likely cause instead of the bare word "error"', async () => {
  const { ctx } = await harness({ partStartStaggerMs: 0 })
  ctx.provide('subagents', {
    start: async () => ({
      id: 'child-stuck',
      localAgent: undefined,
      result: Promise.resolve({ output: [], stopReason: 'error' }),
      dispose: async () => {},
    }),
  } as never)
  const tool = ctx.tools.get(RUN_WAVE)
  assert.ok(tool !== undefined)
  const session = ctx.sessions.create(SessionId('stuck'))
  const value = await tool.execute({
    parts: [{ id: 'core', task: 'Write the core module.', files: ['core.py'] }],
  }, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never) as {
    parts: { id: string; status: string; summary: string }[]
  }
  const part = value.parts[0]
  assert.equal(part?.status, 'failed')
  assert.notEqual(part?.summary, 'error', 'the bare stopReason alone helps nobody')
  assert.match(part?.summary ?? '', /retry/u)
  assert.match(part?.summary ?? '', /rate limit|throttle/u)
})

test('harness_report reads the same budget the pre-step listener enforces, before any run_wave', async () => {
  const { ctx, spend, step, session } = await harness({ maxTokens: 1_000, stepsPerTurn: 10, cacheReadWeight: 1 })
  await step(3)
  spend.uncachedInputTokens = 250
  const tool = ctx.tools.get(HARNESS_REPORT)
  assert.ok(tool !== undefined, 'harness_report must be registered')
  const value = await tool.execute({}, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never) as {
    budget: { tokensSpent: number; tokensLimit: number; stepsUsed: number; stepsLimit: number }
    waveRuns: { total: number; failed: number }
    findings: { severity: string; summary: string }[]
  }
  assert.equal(value.budget.tokensSpent, 250)
  assert.equal(value.budget.tokensLimit, 1_000)
  assert.equal(value.budget.stepsUsed, 3)
  assert.equal(value.budget.stepsLimit, 10)
  assert.equal(value.waveRuns.total, 0)
  assert.equal(value.findings[0]?.severity, 'low')
})

test('harness_report surfaces a failed run_wave part as a finding, in the same session that ran it', async () => {
  const { ctx, session } = await harness({ partStartStaggerMs: 0 })
  ctx.provide('subagents', {
    start: async (_provider: string, request: { label: string }) => ({
      id: `child-${request.label}`,
      localAgent: undefined,
      result: Promise.resolve(request.label.endsWith(':docs')
        ? { output: [], stopReason: 'completed', structured: { status: 'failed', summary: 'blocked on the name.' } }
        : { output: [], stopReason: 'completed', structured: { status: 'done', summary: 'ok.' } }),
      dispose: async () => {},
    }),
  } as never)
  const runWave = ctx.tools.get(RUN_WAVE)
  assert.ok(runWave !== undefined)
  await runWave.execute({
    parts: [
      { id: 'core', task: 'Write the core module.', files: ['core.py'] },
      { id: 'docs', task: 'Write the README.', files: ['README.md'] },
    ],
  }, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never)
  const tool = ctx.tools.get(HARNESS_REPORT)
  assert.ok(tool !== undefined)
  const value = await tool.execute({}, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never) as {
    waveRuns: { total: number; failed: number }
    findings: { severity: string; summary: string; detail: string }[]
  }
  assert.equal(value.waveRuns.total, 2)
  assert.equal(value.waveRuns.failed, 1)
  const finding = value.findings.find(candidate => candidate.severity === 'high')
  assert.ok(finding !== undefined)
  assert.match(finding.detail, /docs \(wave 1\): blocked on the name\./u)
})

test('harness_report never writes anything and never changes the budget it reports on', async () => {
  const { ctx, session } = await harness()
  const tool = ctx.tools.get(HARNESS_REPORT)
  assert.ok(tool !== undefined)
  const first = await tool.execute({}, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never)
  const second = await tool.execute({}, { agent: { session } as unknown as Agent, signal: new AbortController().signal } as never)
  assert.deepEqual(first, second)
})
