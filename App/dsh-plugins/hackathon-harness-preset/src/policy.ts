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
  '1. The frozen session spec is the job. Build what it asks and nothing it does not; never widen it on your own.',
  '2. Decompose before editing. When a job has two or more parts, call wave_plan with each part and the files it will'
    + ' touch, then work the waves in order. Parts inside one wave are independent and may run together (subagent).',
  '3. Prove before you build: give each subtask a check you can run (a test, a command, a diff assertion) before writing'
    + ' its implementation, and run that check again after.',
  '4. No runnable check for a subtask means the subtask is unverified: convene committee on it before reporting it done,'
    + ' and act on the quorum verdict.',
  '5. A failed check re-enters composition for that subtask alone. Never redo work a passing check already proved.',
  '6. The harness enforces the budget itself. When it warns you, land the subtask in flight and report — do not start'
    + ' anything new.',
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
