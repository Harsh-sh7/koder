/**
 * The self-check report is only trustworthy if its findings track the exact
 * thresholds the budget itself enforces (`decideBudget` in `budget.ts`), and if
 * a `run_wave` failure always surfaces rather than getting summarized away. No
 * dsh runtime is involved here: `buildReport` and `renderReport` are pure
 * functions of the same numbers `index.ts` already tracks.
 */

import { strict as assert } from 'node:assert'
import { test } from 'node:test'

import { buildReport, renderReport } from '../src/review.ts'
import type { BudgetSnapshot, WaveOutcomeLog } from '../src/review.ts'

const okBudget: BudgetSnapshot = {
  tokensSpent: 100,
  tokensLimit: 1_000,
  stepsUsed: 2,
  stepsLimit: 10,
  escalations: 0,
  maxEscalations: 2,
}

test('a turn within every ceiling reports one low-severity finding and no failed waves', () => {
  const report = buildReport(okBudget, [])
  assert.equal(report.findings.length, 1)
  assert.equal(report.findings[0]?.severity, 'low')
  assert.equal(report.waveRuns.total, 0)
  assert.equal(report.waveRuns.failed, 0)
})

test('token spend at or past warnAt is medium; at or past the ceiling is high', () => {
  const near = buildReport({ ...okBudget, tokensSpent: 800 }, [])
  assert.equal(near.findings[0]?.severity, 'medium')
  assert.match(near.findings[0]?.summary ?? '', /nearly exhausted/u)

  const over = buildReport({ ...okBudget, tokensSpent: 1_000 }, [])
  assert.equal(over.findings[0]?.severity, 'high')
  assert.match(over.findings[0]?.summary ?? '', /^Token budget exhausted$/u)
})

test('step usage is graded the same way as token spend, independently', () => {
  const near = buildReport({ ...okBudget, stepsUsed: 8 }, [])
  assert.ok(near.findings.some(finding => finding.severity === 'medium' && /Step cap nearly reached/u.test(finding.summary)))

  const over = buildReport({ ...okBudget, stepsUsed: 10 }, [])
  assert.ok(over.findings.some(finding => finding.severity === 'high' && /Step cap reached/u.test(finding.summary)))
})

test('any escalation used is a finding, and using the last one is high severity', () => {
  const one = buildReport({ ...okBudget, escalations: 1, maxEscalations: 2 }, [])
  assert.ok(one.findings.some(finding => finding.severity === 'medium' && /escalation/iu.test(finding.summary)))

  const last = buildReport({ ...okBudget, escalations: 2, maxEscalations: 2 }, [])
  assert.ok(last.findings.some(finding => finding.severity === 'high' && /escalation/iu.test(finding.summary)))
})

test('a failed run_wave part is always a high-severity finding naming the part and its summary', () => {
  const waves: WaveOutcomeLog[] = [
    { id: 'core', wave: 1, status: 'done', summary: 'core ok.' },
    { id: 'docs', wave: 1, status: 'failed', summary: 'blocked on the name.' },
  ]
  const report = buildReport(okBudget, waves)
  assert.equal(report.waveRuns.total, 2)
  assert.equal(report.waveRuns.failed, 1)
  const finding = report.findings.find(candidate => candidate.severity === 'high')
  assert.ok(finding !== undefined, 'a failed part must produce a finding')
  assert.match(finding.summary, /1 of 2 run_wave part\(s\) failed/u)
  assert.match(finding.detail, /docs \(wave 1\): blocked on the name\./u)
})

test('findings are ordered worst first', () => {
  const waves: WaveOutcomeLog[] = [{ id: 'docs', wave: 1, status: 'failed', summary: 'x' }]
  const report = buildReport({ ...okBudget, tokensSpent: 800 }, waves)
  assert.deepEqual(report.findings.map(finding => finding.severity), ['high', 'medium'])
})

test('the rendered report carries the budget line, the wave line, and every finding', () => {
  const waves: WaveOutcomeLog[] = [{ id: 'docs', wave: 1, status: 'failed', summary: 'blocked.' }]
  const text = renderReport(buildReport(okBudget, waves))
  assert.match(text, /Budget: 100\/1000 weighted tokens, step 2\/10, 0\/2 escalations used\./u)
  assert.match(text, /Delegated parts this turn: 1 run, 1 failed\./u)
  assert.match(text, /- \[high\] 1 of 1 run_wave part\(s\) failed — docs \(wave 1\): blocked\./u)
})

test('a zero-limit budget never divides by zero into a false finding', () => {
  const report = buildReport({ ...okBudget, tokensLimit: 0, stepsLimit: 0 }, [])
  assert.equal(report.findings.length, 1)
  assert.equal(report.findings[0]?.severity, 'low')
})
