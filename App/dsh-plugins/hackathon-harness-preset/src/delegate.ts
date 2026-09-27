/**
 * `run_wave`: the harness, not the model, runs a decomposed job's parts as
 * parallel subagents.
 *
 * Leaving delegation to the model meant it rarely happened — a model has to
 * call `wave_plan`, then issue one `subagent` call per part, then wait on each,
 * and small models skip the whole dance and do everything serially in one
 * context. One `run_wave` call replaces that: the harness partitions the parts
 * into file-disjoint waves (the same `planWaves` arithmetic), seats every part
 * of a wave as a fresh child at once, waits for the wave, and returns one
 * compact result per part. The parent pays one round trip for the whole job,
 * and each child reads only what its own part needs.
 *
 * Everything here is pure: the prompt a child receives, the ownership rule the
 * harness enforces on a child's file writes, and the text the parent reads.
 *
 * @module @harness/dsh-hackathon-harness-preset/delegate
 */

import { isAbsolute, relative, resolve } from 'node:path'

import { normalizePath } from './waves.ts'

/** One part of a job, as the model declares it to `run_wave`. */
export interface WavePart {
  /** Stable id, unique in the call. */
  readonly id: string
  /** What the part must do, in the words the child will read. */
  readonly task: string
  /** Workspace-relative files the part owns; its child may write only these. */
  readonly files: readonly string[]
  /** The command that proves the part, when there is one. */
  readonly check?: string
}

/** What one part came back as. */
export interface PartOutcome {
  /** The part's id. */
  readonly id: string
  /** The 1-based wave the part ran in. */
  readonly wave: number
  /** `done` when the child reports its check passing, `failed` otherwise. */
  readonly status: 'done' | 'failed'
  /** The child's own one- or two-sentence account, or why no account came back. */
  readonly summary: string
}

/**
 * The prompt one part's child receives.
 *
 * A child starts with no transcript, so everything it needs to work alone is
 * here — and nothing more, because every line is paid once per part: the job
 * (the frozen spec, or a pointer to it), the part, the files it owns, the
 * check, and the answer shape.
 * @param part - the part being delegated.
 * @param spec - the session's frozen spec text, when one is locked.
 * @param siblings - ids of the other parts running at the same time.
 * @returns the child's full prompt.
 */
export function partPrompt(part: WavePart, spec: string | undefined, siblings: readonly string[]): string {
  const lines: string[] = []
  if (spec !== undefined && spec.trim().length > 0) {
    const text = spec.trim()
    // Fresh children have their own spec projection: spec_get cannot fetch
    // the parent's spec, so a pointer would silently lose its requirements.
    lines.push(`The overall job (context only — you build one part of it):\n${text}`)
    lines.push('')
  }
  lines.push(`Your part, "${part.id}": ${part.task.trim()}`)
  lines.push('')
  if (part.files.length > 0) {
    lines.push(`Files you own: ${part.files.join(', ')}. Write only these — the harness refuses writes to any other file.`)
  } else {
    lines.push('This part owns no file: read, run, and report; do not write files.')
  }
  if (siblings.length > 0) {
    lines.push(`Other agents are working on ${siblings.join(', ')} at the same time; if your part needs a change in their`
      + ' files, do not make it — say so in your summary.')
  }
  lines.push(part.check !== undefined && part.check.trim().length > 0
    ? `Prove it: \`${part.check.trim()}\` must pass. Run it before you finish.`
    : 'Prove it with the smallest command that exercises your part, and run it before you finish.')
  lines.push('Work tightly: read only what you need, change nothing outside your part, no reformatting or refactors.')
  lines.push('Finish with the result schema: status (done only when your check passed) and a one- or two-sentence summary'
    + ' naming the files you changed and the check result.')
  return lines.join('\n')
}

/** The structured answer every part's child must return. */
export const PART_RESULT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    status: {
      type: 'string',
      enum: ['done', 'failed'],
      description: 'done only when the part is built and its check passed; failed otherwise.',
    },
    summary: {
      type: 'string',
      description: 'One or two sentences: the files changed and the check result, or what blocked the part.',
    },
  },
  required: ['status', 'summary'],
} as const

/**
 * Whether a child that owns `owned` may write `target`.
 *
 * The rule is the plan's own: a file is written by the one part that declared
 * it, so parallel children never race on a file. A declared directory (a path
 * ending in `/`, or one that is a prefix of the target) owns what is under it.
 * Comparison uses the planner's normalization, so the case-folding and
 * separator rules that made the plan safe are the ones that enforce it.
 * @param owned - the files the part declared.
 * @param target - the path the child is about to write, as the tool received it.
 * @param cwd - the child's working directory, for relative and absolute paths alike.
 * @returns true when the write is the part's own.
 */
export function ownsPath(owned: readonly string[], target: string, cwd: string): boolean {
  const absolute = isAbsolute(target) ? target : resolve(cwd, target)
  const rel = relative(cwd, absolute)
  if (rel === '..' || rel.startsWith('../') || rel.startsWith('..\\') || isAbsolute(rel)) return false
  const wanted = normalizePath(rel)
  return owned.some((declared) => {
    const file = normalizePath(declared)
    return file.length > 0 && (wanted === file || wanted.startsWith(`${file}/`))
  })
}

/**
 * Split a wave's parts into batches of at most `size`, in order.
 *
 * A wave's parts share no file, so they are free to start together — but
 * "free to" is not "safe to" on a route whose account-wide rate limit is
 * small: a shared free-tier key can have a per-minute cap far below what even
 * three or four parts ask for in one burst, in which case every part in the
 * wave fails together, retry or not, because the total the wave asked for in
 * that one minute was never obtainable — no amount of per-part backoff fixes
 * a batch that was too big to begin with. Batching bounds how much of a wave
 * is ever asked for at once; `size <= 0` means no batching (the whole wave is
 * one batch, today's behaviour).
 * @param parts - the wave's parts, in plan order.
 * @param size - the largest batch to run at once; `<= 0` means unlimited.
 * @returns the parts, grouped into batches of at most `size`.
 */
export function batchParts<T>(parts: readonly T[], size: number): T[][] {
  if (size <= 0 || parts.length === 0) return parts.length === 0 ? [] : [[...parts]]
  const batches: T[][] = []
  for (let start = 0; start < parts.length; start += size) batches.push([...parts.slice(start, start + size)])
  return batches
}

/**
 * The run as the parent model reads it: one line per part, grouped by wave,
 * and the one instruction the parent still owes — its own verification.
 * @param outcomes - every part's outcome, in wave order.
 * @param waves - how many waves ran.
 * @returns the tool's text result.
 */
export function renderRun(outcomes: readonly PartOutcome[], waves: number): string {
  const done = outcomes.filter(outcome => outcome.status === 'done').length
  const lines = [`${String(done)} of ${String(outcomes.length)} parts done in ${String(waves)} wave(s):`]
  for (const outcome of outcomes) {
    lines.push(`  [wave ${String(outcome.wave)}] ${outcome.id}: ${outcome.status} — ${outcome.summary}`)
  }
  lines.push('', done === outcomes.length
    ? 'Verify before reporting: run the checks yourself (one combined command is cheapest).'
    : 'Fix the failed parts yourself (their files are free now), then run every check before reporting.')
  return lines.join('\n')
}
