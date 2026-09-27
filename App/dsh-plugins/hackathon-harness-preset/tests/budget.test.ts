/**
 * The budget decides when the harness stops work, so its boundaries are the
 * contract: exactly where a warning starts, exactly where a stop begins, which
 * stop outranks the other, and that the same spend always prints the same
 * sentence (the model reads these as instructions, and a locale-dependent
 * count would make one ceiling read as two).
 */

import { strict as assert } from 'node:assert'
import { test } from 'node:test'

import { decideBudget, formatCount, totalTokens, weightedTokens } from '../src/budget.ts'
import type { BudgetLimits } from '../src/budget.ts'

const limits: BudgetLimits = { tokens: 1_000, stepsPerTurn: 5, warnAt: 0.8 }

/** The decision for one spend, with the turn's step count defaulted to one. */
function decide(tokens: number, stepsInTurn = 1, overrides: Partial<BudgetLimits> = {}) {
  return decideBudget({ ...limits, ...overrides }, { tokens, stepsInTurn })
}

test('every bucket counts toward the spend', () => {
  assert.equal(totalTokens({ uncachedInputTokens: 10, outputTokens: 20, cacheReadTokens: 300, cacheWriteTokens: 4 }), 334)
  assert.equal(totalTokens({ uncachedInputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 }), 0)
})

test('cache reads count at their weight, every other bucket in full', () => {
  const totals = { uncachedInputTokens: 10, outputTokens: 20, cacheReadTokens: 300, cacheWriteTokens: 4 }
  assert.equal(weightedTokens(totals, 0.1), 64)
  assert.equal(weightedTokens(totals, 1), totalTokens(totals))
  assert.equal(weightedTokens(totals, 0), 34)
})

test('counts print the same digits on every host', () => {
  assert.equal(formatCount(0), '0')
  assert.equal(formatCount(999), '999')
  assert.equal(formatCount(1_000), '1,000')
  assert.equal(formatCount(1_234_567), '1,234,567')
})

test('nothing happens under the warning line', () => {
  assert.deepEqual(decide(0), { kind: 'ok' })
  assert.deepEqual(decide(799), { kind: 'ok' })
})

test('the warning line is the fraction of the live ceiling, and it warns once', () => {
  const atLine = decide(800)
  assert.equal(atLine.kind, 'warn')
  assert.match(atLine.kind === 'warn' ? atLine.text : '', /80% spent \(800 of 1,000 model tokens\)/u)
  // The text is an instruction, not a status line: the model is the only actor
  // that can still land the subtask in flight.
  assert.match(atLine.kind === 'warn' ? atLine.text : '', /Finish the subtask in progress and report; do not start new work\.$/u)
  // A raised ceiling moves the line with it rather than warning against the old one.
  assert.equal(decide(800, 1, { tokens: 2_000 }).kind, 'ok')
})

test('the ceiling outranks the warning and the step cap', () => {
  const spent = decide(1_000, 99)
  assert.equal(spent.kind, 'stop')
  assert.equal(spent.kind === 'stop' ? spent.over : undefined, 'tokens')
  assert.match(spent.kind === 'stop' ? spent.text : '', /Session budget exhausted: 1,000 of 1,000 model tokens spent/u)
  // A turn-scoped ceiling names itself, so the stop reads as what it is.
  const turn = decide(1_000, 1, { scope: 'turn' })
  assert.match(turn.kind === 'stop' ? turn.text : '', /^Turn budget exhausted/u)
})

test('the step cap warns at the same fraction, and a token warning outranks it', () => {
  // 80% of a 5-step turn is step 4: the model is told to converge before the cut.
  assert.equal(decide(0, 3).kind, 'ok')
  const late = decide(0, 4)
  assert.equal(late.kind, 'warn')
  assert.equal(late.kind === 'warn' ? late.over : undefined, 'steps')
  assert.match(late.kind === 'warn' ? late.text : '', /step 4 of a 5-step turn\. Converge now/u)
  const both = decide(800, 4)
  assert.equal(both.kind === 'warn' ? both.over : undefined, 'tokens')
})

test('the turn step cap stops one turn and the last allowed step still runs', () => {
  assert.notEqual(decide(0, 5).kind, 'stop')
  const over = decide(0, 6)
  assert.equal(over.kind, 'stop')
  assert.equal(over.kind === 'stop' ? over.over : undefined, 'steps')
  assert.match(over.kind === 'stop' ? over.text : '', /step 6 of a 5-step turn/u)
})
