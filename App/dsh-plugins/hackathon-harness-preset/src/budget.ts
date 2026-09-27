/**
 * Budget arithmetic and the one decision the harness takes on it: carry on,
 * warn the model, or stop the turn.
 *
 * A budget is deliberately counted in *model tokens* — every token the model
 * read or wrote through this session — rather than in money. Pricing changes
 * per route and per cache hit, and a harness that stops work because a
 * discounted token became an expensive one would be unpredictable; a token
 * count is the same number on every route, and the application prices it
 * separately when it shows a cost. The cap exists to bound the *work*, so the
 * warning is phrased as an instruction to finish rather than as a status line:
 * the model is the only actor that can still land the current subtask.
 *
 * @module @harness/dsh-hackathon-harness-preset/budget
 */

/**
 * The counter shape the session's durable usage folds into — the same four
 * buckets the token meter publishes, so a reader never pays to re-price a
 * request to answer "how much has this session spent".
 */
export interface TokenTotals {
  /** Input tokens the route did not serve from its prompt cache. */
  readonly uncachedInputTokens: number
  /** Output tokens the model generated. */
  readonly outputTokens: number
  /** Input tokens served from the route's prompt cache. */
  readonly cacheReadTokens: number
  /** Input tokens written into the route's prompt cache. */
  readonly cacheWriteTokens: number
}

/** The live ceiling of one budget. */
export interface BudgetLimits {
  /** Model tokens the budget's scope may spend in total. */
  readonly tokens: number
  /** Model steps one turn may take before the harness stops it. */
  readonly stepsPerTurn: number
  /** Fraction of {@link BudgetLimits.tokens} (and of the step cap) at which the model is warned once. */
  readonly warnAt: number
  /** What the token ceiling covers, as the stop sentence names it. Defaults to `session`. */
  readonly scope?: 'turn' | 'session'
}

/** What the budget's scope has spent so far. */
export interface BudgetUsage {
  /** Model tokens spent in the budget's scope, from {@link weightedTokens}. */
  readonly tokens: number
  /** Model steps this turn has taken. */
  readonly stepsInTurn: number
}

/**
 * What the harness does with the step the agent is proposing.
 *
 * `'warn'` carries the model-facing message the caller admits into the step;
 * `'stop'` carries the sentence the caller records — for the human when it
 * escalates, and for the log when it does not — because a stopped turn ends
 * before the model could read anything.
 */
export type BudgetDecision =
  | { readonly kind: 'ok' }
  | { readonly kind: 'warn'; readonly over: 'tokens' | 'steps'; readonly text: string }
  | { readonly kind: 'stop'; readonly over: 'tokens' | 'steps'; readonly text: string }

/**
 * Sum every bucket into the one number a budget counts.
 * @param totals - the session's durable usage buckets.
 * @returns the total model tokens the session has spent.
 */
export function totalTokens(totals: TokenTotals): number {
  return totals.uncachedInputTokens + totals.outputTokens + totals.cacheReadTokens + totals.cacheWriteTokens
}

/**
 * The spend a budget counts: every bucket, with prompt-cache reads weighted.
 *
 * An agent loop re-sends its whole prefix on every step, so on a route with a
 * prompt cache most input tokens are cache reads — billed and computed at a
 * small fraction of fresh input. Counting them at full weight made a budget
 * that stopped a turn after a dozen steps of perfectly cheap work; weighting
 * them keeps the cap on the work that actually costs.
 * @param totals - the usage buckets.
 * @param cacheReadWeight - what one cache-read token counts as, 0 to 1.
 * @returns the weighted spend.
 */
export function weightedTokens(totals: TokenTotals, cacheReadWeight: number): number {
  return totals.uncachedInputTokens + totals.outputTokens + totals.cacheWriteTokens
    + totals.cacheReadTokens * cacheReadWeight
}

/**
 * Format a count with thousands separators, deterministically.
 *
 * `toLocaleString` is not usable here: the same budget must print the same
 * sentence on every host, and the model reads these sentences framed as
 * instructions.
 * @param value - the non-negative integer to format.
 * @returns the digits with commas between groups of three.
 */
export function formatCount(value: number): string {
  return String(Math.round(value)).replace(/\B(?=(\d{3})+(?!\d))/gu, ',')
}

/**
 * Decide what happens to the step being proposed.
 *
 * A stop outranks a warning, and the token ceiling outranks the step cap: the
 * token ceiling is the one the user set for the whole session, while the step
 * cap only bounds one turn.
 * @param limits - the session's live ceiling.
 * @param usage - what the session and the current turn have spent.
 * @returns the decision, with the text the caller either admits or records.
 */
export function decideBudget(limits: BudgetLimits, usage: BudgetUsage): BudgetDecision {
  const scope = limits.scope === 'turn' ? 'Turn' : 'Session'
  if (usage.tokens >= limits.tokens) {
    return {
      kind: 'stop',
      over: 'tokens',
      text: `${scope} budget exhausted: ${formatCount(usage.tokens)} of ${formatCount(limits.tokens)} model tokens spent`
        + ` (${formatCount(usage.stepsInTurn)} steps into this turn).`,
    }
  }
  if (usage.stepsInTurn > limits.stepsPerTurn) {
    return {
      kind: 'stop',
      over: 'steps',
      text: `Turn step cap reached: step ${formatCount(usage.stepsInTurn)} of a ${formatCount(limits.stepsPerTurn)}-step turn.`,
    }
  }
  const warningAt = limits.warnAt * limits.tokens
  if (usage.tokens >= warningAt) {
    const percent = Math.floor((usage.tokens / limits.tokens) * 100)
    return {
      kind: 'warn',
      over: 'tokens',
      text: `Harness budget: ${percent}% spent (${formatCount(usage.tokens)} of ${formatCount(limits.tokens)} model tokens).`
        + ' Finish the subtask in progress and report; do not start new work.',
    }
  }
  // The step cap warns too: a turn that is simply stopped mid-edit leaves a
  // half-applied change and no report, which is the worst way for a turn to end.
  const stepWarningAt = Math.max(1, Math.floor(limits.warnAt * limits.stepsPerTurn))
  if (usage.stepsInTurn >= stepWarningAt) {
    return {
      kind: 'warn',
      over: 'steps',
      text: `Harness budget: step ${formatCount(usage.stepsInTurn)} of a ${formatCount(limits.stepsPerTurn)}-step turn.`
        + ' Converge now: run the check that proves the change, then report; do not start new exploration.',
    }
  }
  return { kind: 'ok' }
}
