/**
 * File-conflict wave partitioner: turn a declared subtask list into ordered
 * waves in which no two subtasks share a file.
 *
 * The partitions are the harness's guarantee, not the model's judgement. A plan
 * is a pure function of the declared subtasks, so the same list always produces
 * the same waves, and a subtask pair that overlaps is never placed in one wave —
 * which is what makes parallel execution safe without a lock, a worktree, or a
 * re-read of the tree after every edit. Subtasks that declare no files are
 * reported rather than assumed harmless: the declaration is the contract, and a
 * caller that cannot name a subtask's files cannot claim it is non-conflicting.
 *
 * @module @harness/dsh-hackathon-harness-preset/waves
 */

/** One subtask exactly as the planner declares it. */
export interface Subtask {
  /** Stable id the plan and the caller's own tracking both use. */
  readonly id: string
  /**
   * Workspace-relative files this subtask may write. An empty list declares a
   * subtask that touches no file of its own; see {@link WavePlan.undeclared}.
   */
  readonly files: readonly string[]
}

/** One wave: the subtasks that may run at the same time. */
export interface Wave {
  /** 1-based position in the plan; earlier waves are ordered before later ones. */
  readonly index: number
  /** Member subtask ids, in the order they were declared. */
  readonly subtasks: string[]
  /** Whether the wave holds more than one subtask and therefore has anything to parallelize. */
  readonly parallel: boolean
}

/** One file that more than one subtask declares. */
export interface FileConflict {
  /** The normalized file path. */
  readonly file: string
  /** Ids of every subtask that declares it, in declaration order. */
  readonly subtasks: string[]
}

/** A complete partitioning of one subtask list. */
export interface WavePlan {
  /** Ordered waves; every declared subtask appears in exactly one. */
  readonly waves: Wave[]
  /** Every file that two or more subtasks declare, sorted by path. */
  readonly conflicts: FileConflict[]
  /** Ids of subtasks that declared no files, in declaration order. */
  readonly undeclared: string[]
  /**
   * Number of waves — the length of the longest chain of file-overlapping work,
   * so it is also the number of subtask rounds a fully sequential execution of
   * this plan would take.
   */
  readonly depth: number
}

/** Refusal raised for a subtask list that cannot be planned. */
export class WavePlanError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'WavePlanError'
  }
}

/**
 * Normalize one declared path into the identity conflicts are compared under.
 *
 * Two normalizations beyond trimming are load-bearing. Separators collapse to
 * `/`, because a plan written on one platform is planned on another. Case folds,
 * because the same workspace is routinely reached through a case-insensitive
 * filesystem, where two spellings are one file — folding can only ever report
 * *more* conflicts, so a plan stays safe everywhere it runs.
 * @param path - the declared path.
 * @returns the comparison identity of that path.
 */
export function normalizePath(path: string): string {
  return path.trim().replaceAll('\\', '/').replace(/\/{2,}/gu, '/').replace(/^\.\//u, '').replace(/\/+$/u, '').toLowerCase()
}

/**
 * Partition subtasks into ordered, conflict-free waves.
 *
 * Each subtask joins the earliest wave whose members share no file with it, and
 * starts a new wave when every earlier wave is blocked. A subtask that overlaps
 * earlier work therefore lands in a later wave and runs after it, while
 * independent subtasks keep landing in the earliest wave they do not block —
 * that earliest-fit rule is what keeps the wave count (and so the serial
 * rounds) minimal without any search.
 * @param subtasks - declared subtasks, in the order the caller intends to explain them.
 * @returns the plan: ordered waves, the files behind every forced ordering, and the subtasks that declared no files.
 * @throws {WavePlanError} when the list is empty, an id is blank or repeated, or a declared file is blank.
 */
export function planWaves(subtasks: readonly Subtask[]): WavePlan {
  if (subtasks.length === 0) {
    throw new WavePlanError('planWaves: at least one subtask is required')
  }
  const ids = new Set<string>()
  const normalized: { id: string; files: readonly string[] }[] = []
  for (const subtask of subtasks) {
    const id = subtask.id.trim()
    if (id.length === 0) throw new WavePlanError('planWaves: every subtask needs a non-empty id')
    if (ids.has(id)) throw new WavePlanError(`planWaves: subtask id "${id}" is declared more than once`)
    ids.add(id)
    const files: string[] = []
    for (const declared of subtask.files) {
      const file = normalizePath(declared)
      if (file.length === 0) {
        throw new WavePlanError(`planWaves: subtask "${id}" declares a blank file path; omit the file or name it`)
      }
      if (!files.includes(file)) files.push(file)
    }
    normalized.push({ id, files })
  }

  const waves: { files: Set<string>; subtasks: string[] }[] = []
  const undeclared: string[] = []
  for (const subtask of normalized) {
    if (subtask.files.length === 0) undeclared.push(subtask.id)
    const wave = waves.find(candidate => subtask.files.every(file => !candidate.files.has(file)))
    if (wave === undefined) {
      waves.push({ files: new Set(subtask.files), subtasks: [subtask.id] })
      continue
    }
    for (const file of subtask.files) wave.files.add(file)
    wave.subtasks.push(subtask.id)
  }

  const owners = new Map<string, string[]>()
  for (const subtask of normalized) {
    for (const file of subtask.files) {
      const list = owners.get(file)
      if (list === undefined) owners.set(file, [subtask.id])
      else list.push(subtask.id)
    }
  }
  const conflicts: FileConflict[] = [...owners]
    .filter(([, list]) => list.length > 1)
    .map(([file, list]) => ({ file, subtasks: list }))
    .sort((left, right) => left.file < right.file ? -1 : left.file > right.file ? 1 : 0)

  return {
    waves: waves.map((wave, index) => ({
      index: index + 1,
      subtasks: wave.subtasks,
      parallel: wave.subtasks.length > 1,
    })),
    conflicts,
    undeclared,
    depth: waves.length,
  }
}
