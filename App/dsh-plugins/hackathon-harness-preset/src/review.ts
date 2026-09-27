/**
 * The turn's own self-check: a small, evidence-bound findings report built
 * from data the harness already tracks — budget spend, step usage,
 * escalations, and how the parts of any `run_wave` this turn ran came back.
 *
 * This is deliberately not a review of the code the turn wrote (the committee
 * exists for that) and not a new source of truth: every field here is read
 * from state `index.ts` already maintains for the budget and for `run_wave`,
 * so the report can never disagree with the decisions the harness actually
 * took. Its only job is to turn those numbers into a few sentences a model
 * (or a person reading the transcript) can act on without re-deriving them —
 * "is this turn in trouble?" answered by evidence instead of a guess.
 *
 * @module @harness/dsh-hackathon-harness-preset/review
 */

/** One `run_wave` part's outcome, as much of it as a finding needs. */
export interface WaveOutcomeLog {
  /** The part's id. */
  readonly id: string
  /** The 1-based wave it ran in. */
  readonly wave: number
  /** `done` when the part's child reported its check passing, `failed` otherwise. */
  readonly status: 'done' | 'failed'
  /** The child's own account, or why no account came back. */
  readonly summary: string
}

/** The turn's budget state, already computed by the `agent/pre-step` listener. */
export interface BudgetSnapshot {
  /** Weighted model tokens spent in the budget's current scope. */
  readonly tokensSpent: number
  /** The live token ceiling, including any granted escalations. */
  readonly tokensLimit: number
  /** Steps the current turn has taken. */
  readonly stepsUsed: number
  /** The live step cap, including any granted extra steps. */
  readonly stepsLimit: number
  /** Escalations already approved this session. */
  readonly escalations: number
  /** Escalations one session may take. */
  readonly maxEscalations: number
}

/** One observation in the report, ranked so the worst reads first. */
export interface Finding {
  /** How much this needs attention before the turn is trusted as done. */
  readonly severity: 'high' | 'medium' | 'low'
  /** The claim, in one line. */
  readonly summary: string
  /** The evidence behind it. */
  readonly detail: string
}

/** The self-check, built once from a budget snapshot and this turn's wave history. */
export interface TurnReport {
  readonly budget: BudgetSnapshot
  readonly waveRuns: { readonly total: number; readonly failed: number }
  // A plain (non-readonly) array: this is what a tool's execute() hands back
  // as its structured result, and the runtime's own output type for an array
  // schema is never `readonly` — a readonly field here would only fight that.
  readonly findings: Finding[]
}

/** Fraction of a limit at which spend is called "nearly exhausted" rather than merely spent. */
const NEAR_LIMIT = 0.8

/**
 * Turn a budget snapshot and a turn's `run_wave` history into findings.
 *
 * Every threshold here mirrors the one `decideBudget` (`budget.ts`) already
 * enforces — `warnAt` for "nearly exhausted", the ceiling itself for
 * "exhausted" — so a reader of this report and a reader of the budget's own
 * warning message never see two different opinions about the same turn.
 * @param budget - the turn's current spend against its live ceilings.
 * @param waves - every `run_wave` part outcome recorded so far this turn.
 * @returns the report, worst finding first.
 */
export function buildReport(budget: BudgetSnapshot, waves: readonly WaveOutcomeLog[]): TurnReport {
  const findings: Finding[] = []

  const tokenFraction = budget.tokensLimit > 0 ? budget.tokensSpent / budget.tokensLimit : 0
  if (tokenFraction >= 1) {
    findings.push({
      severity: 'high',
      summary: 'Token budget exhausted',
      detail: `${String(Math.round(budget.tokensSpent))} of ${String(Math.round(budget.tokensLimit))} weighted`
        + ' model tokens spent for this scope; the harness will stop the next step unless a grant is escalated.',
    })
  } else if (tokenFraction >= NEAR_LIMIT) {
    findings.push({
      severity: 'medium',
      summary: 'Token budget nearly exhausted',
      detail: `${String(Math.floor(tokenFraction * 100))}% of the ${String(Math.round(budget.tokensLimit))}-token`
        + ' ceiling spent; land the subtask in progress before starting new work.',
    })
  }

  const stepFraction = budget.stepsLimit > 0 ? budget.stepsUsed / budget.stepsLimit : 0
  if (stepFraction >= 1) {
    findings.push({
      severity: 'high',
      summary: 'Step cap reached',
      detail: `Step ${String(budget.stepsUsed)} of a ${String(budget.stepsLimit)}-step turn.`,
    })
  } else if (stepFraction >= NEAR_LIMIT) {
    findings.push({
      severity: 'medium',
      summary: 'Step cap nearly reached',
      detail: `Step ${String(budget.stepsUsed)} of a ${String(budget.stepsLimit)}-step turn; converge and report soon.`,
    })
  }

  if (budget.escalations > 0) {
    findings.push({
      severity: budget.escalations >= budget.maxEscalations ? 'high' : 'medium',
      summary: 'Budget escalation used',
      detail: `${String(budget.escalations)} of ${String(budget.maxEscalations)} approved escalations spent this`
        + ' session; a turn that keeps needing more room is a sign the job should have been split smaller.',
    })
  }

  const failed = waves.filter(part => part.status === 'failed')
  if (failed.length > 0) {
    findings.push({
      severity: 'high',
      summary: `${String(failed.length)} of ${String(waves.length)} run_wave part(s) failed`,
      detail: failed.map(part => `${part.id} (wave ${String(part.wave)}): ${part.summary}`).join(' | '),
    })
  }

  if (findings.length === 0) {
    findings.push({
      severity: 'low',
      summary: 'No issues observed',
      detail: waves.length > 0
        ? `Budget and all ${String(waves.length)} delegated part(s) are within bounds for this turn.`
        : 'Budget is within bounds for this turn; no run_wave parts have run yet.',
    })
  }

  // Worst first: a model skimming the report should hit the thing to act on
  // before the reassurance that everything else is fine.
  const order: Record<Finding['severity'], number> = { high: 0, medium: 1, low: 2 }
  findings.sort((left, right) => order[left.severity] - order[right.severity])

  return { budget, waveRuns: { total: waves.length, failed: failed.length }, findings }
}

/**
 * Render a report as the model (or a transcript reader) sees it.
 * @param report - the report from {@link buildReport}.
 * @returns plain text, one finding per line.
 */
export function renderReport(report: TurnReport): string {
  const { budget, waveRuns } = report
  const lines = [
    `Budget: ${String(Math.round(budget.tokensSpent))}/${String(Math.round(budget.tokensLimit))} weighted tokens,`
      + ` step ${String(budget.stepsUsed)}/${String(budget.stepsLimit)},`
      + ` ${String(budget.escalations)}/${String(budget.maxEscalations)} escalations used.`,
    `Delegated parts this turn: ${String(waveRuns.total)} run, ${String(waveRuns.failed)} failed.`,
    '',
    'Findings:',
  ]
  for (const finding of report.findings) lines.push(`- [${finding.severity}] ${finding.summary} — ${finding.detail}`)
  return lines.join('\n')
}
