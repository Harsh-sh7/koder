/**
 * The harness's written policy: the one prompt section every step carries, the
 * wording of its two tools, and the reviewer personas the committee convenes.
 *
 * Everything here is *constant*, and that is a token decision rather than a
 * stylistic one. The harness's prompt prefix is cacheable only while it stays
 * byte-identical between steps, so a policy that embedded live numbers — spend,
 * step counts, the budget ceiling — would invalidate the cache on every step and
 * charge the whole prompt again as fresh input. Live numbers travel in the
 * messages that need them (the budget warning, the escalation prompt), where
 * they cost one line instead of a cache miss.
 *
 * @module @harness/dsh-hackathon-harness-preset/policy
 */

/** Prompt-section name of the operating contract, unique across the composition. */
export const CONTRACT_SECTION = 'harness:operating-contract'

/**
 * The operating contract: what makes this composition a harness rather than a
 * chat with tools. It is deliberately short — it rides every request — and every
 * line is one of the behaviours the composition otherwise cannot guarantee.
 */
const CONTRACT = [
  'Operating contract of this harness:',
  '1. The frozen spec — the task you were most recently given — is the job. Build what it asks and nothing it does not;'
    + ' never widen it on your own.',
  '2. Starting a new project or a substantial new feature (a web app, a service, anything with more than a couple of'
    + ' files): before writing any file, write a short plan — the stack, the folder layout, the modules and what each'
    + ' owns, the data flow — as a few lines in your reply or a short PLAN.md. Then create files in that structure, not'
    + ' ad hoc. Skip this for a small fix in an existing project; the plan is the thing that makes generated projects'
    + ' look hand-organised instead of dumped.',
  '3. Decompose and delegate BEFORE writing a single file, whenever the plan from rule 2 names more than one file to'
    + ' create. An implementation plus its tests is already two parts, and a README makes three — this is the normal'
    + ' shape of almost every build task, not a special case, and it applies however small or simple the task reads.'
    + ' Call run_wave exactly once with every part named in your plan (its task, the files it owns, its check — e.g.'
    + ' "core", "tests", "docs") and let the harness run them as parallel subagents; do not ask the user for permission'
    + ' first and do not write any of those files yourself. Reserve doing it yourself for a genuine one-file change or a'
    + ' single bug fix, where a second agent would have nothing separate to own.',
  '4. Locate before reading: grep/glob for the names, messages, and paths the task mentions, then read only the regions'
    + ' you need (offset/limit), not whole large files.',
  '5. Reproduce before fixing: when the task reports a bug, run or write the smallest check that shows it (an existing'
    + ' test, a new test in the repo\'s own framework, or a one-off command) and watch it fail.',
  '6. Fix the root cause with the smallest change, in the code the task is about. Never weaken, skip, or delete existing'
    + ' tests to make a check pass, and keep behaviour the task did not mention unchanged.',
  '7. Verify: re-run the reproducing check and the existing tests nearest the change (or, after a run_wave, the checks'
    + ' every part reported); a part is done only when they pass. Read the exit code of every command.',
  '8. Clean up: delete scratch scripts and temporary files you created; keep a new test only if it belongs in the suite.',
  '9. Only when no runnable check can settle a decision, convene committee on that one decision and act on its verdict.'
    + ' A failed check re-enters work for that part alone; never redo work a passing check already proved.',
  '10. When the harness warns about budget or steps, converge: finish and verify the change in flight, then report.',
  '11. End with a short report: what was built or fixed, the files changed, and each check command with its result.',
].join('\n')

/**
 * The operating contract, as the section text.
 * @returns the constant contract text.
 */
export function contractText(): string {
  return CONTRACT
}

/** Model-facing description of the `wave_plan` tool. */
export const WAVE_PLAN_DESCRIPTION = 'Partition a decomposed job into waves that are safe to run in parallel: no wave ever holds two'
  + ' subtasks that declare the same file. Call it once you know each part and its files, before you edit anything.'
  + ' Work the waves in order; the parts inside one wave may be delegated together with subagent.'

/** Model-facing description of the `run_wave` tool. */
export const RUN_WAVE_DESCRIPTION = 'Build a multi-part job with parallel subagents in one call. Give every part its task,'
  + ' the files it owns, and the command that proves it; parts that share no file run at the same time, the rest in'
  + ' later waves, and each part\'s agent may write only its own files. Returns every part\'s status and summary. Use it'
  + ' when a job has two or more parts with different files; do a single part yourself.'

/** Model-facing description of the `committee` tool. */
export const COMMITTEE_DESCRIPTION = 'Put one decision in front of independent reviewers and get their verdicts plus the quorum'
  + ' result. Call it when a subtask has no runnable check that could prove or disprove it, before you report that subtask'
  + ' done. Prefer naming paths and commands over pasting output: reviewers share your workspace and your tools.'

/**
 * One reviewer role: the persona a committee child runs under, and what that
 * child is asked to attack. Roles are deliberately different questions about
 * the same change, so a quorum over them is evidence rather than repetition.
 */
export interface ReviewRole {
  /** Stable role id, reported with the verdict. */
  readonly id: string
  /** Persona template for the child; shadows the deployment persona. */
  readonly persona: string
  /** The question this reviewer answers. */
  readonly directive: string
}

/** The committee's default bench, in the order it is seated. */
export const REVIEW_ROLES: readonly ReviewRole[] = [
  {
    id: 'correctness',
    persona: 'You are a correctness reviewer. You judge one claim about a code change: whether it does what it says for'
      + ' every input it will actually meet, including the empty, the boundary, and the malformed case. You read and you'
      + ' run checks; you never modify the workspace.',
    directive: 'Judge whether the change does what the decision claims, for every input it will meet. Name the case that'
      + ' breaks it, or approve.',
  },
  {
    id: 'adversarial',
    persona: 'You are an adversarial reviewer. You judge one claim about a code change by trying to break it: the input'
      + ' nobody handled, the ordering that races, the failure swallowed, the assumption that holds only in the happy'
      + ' path. You read and you run checks; you never modify the workspace.',
    directive: 'Attack the change. Report the sharpest way it fails, or approve if it survives your attempt.',
  },
  {
    id: 'scope',
    persona: 'You are a scope reviewer for a token-budgeted harness. You judge one claim about a code change by asking'
      + ' whether it is the smallest change that satisfies the frozen spec: no speculative option, no unrequested'
      + ' abstraction, no code the spec did not ask for. You read; you never modify the workspace.',
    directive: 'Judge whether the change is the smallest one the spec allows, and name anything it adds beyond the spec,'
      + ' or approve.',
  },
]

/**
 * The prompt one committee member receives. Every member reads the same decision
 * and evidence and answers a different question, so the vote is over the change
 * rather than over the phrasing.
 * @param role - the role being seated.
 * @param decision - the decision under review.
 * @param evidence - optional paths, commands, or observed output to work from.
 * @returns the child's full prompt.
 */
export function committeePrompt(role: ReviewRole, decision: string, evidence: string | undefined): string {
  const lines = [
    role.persona,
    '',
    `Decision under review: ${decision}`,
  ]
  if (evidence !== undefined && evidence.trim().length > 0) {
    lines.push('', `Evidence you were given: ${evidence.trim()}`)
  }
  lines.push(
    '',
    role.directive,
    '',
    'Work in this workspace: read the files involved and run the checks you need. Do not modify anything.',
    'Answer with the verdict schema: your vote, and one sentence giving the reason it is that vote.',
  )
  return lines.join('\n')
}

/**
 * The structured verdict every committee child must return. Object-rooted and
 * closed, because the parent tallies these values without a model in the loop:
 * a vote the schema rejects is not a vote this tool can count.
 */
export const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    vote: {
      type: 'string',
      enum: ['approve', 'reject'],
      description: 'approve when the change survives your review; reject when you found something that must change.',
    },
    reason: { type: 'string', description: 'One sentence: the failure you found, or why it holds.' },
  },
  required: ['vote', 'reason'],
} as const
