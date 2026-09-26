/**
 * The wave plan is what makes parallel delegation safe without a lock: these
 * tests pin the four properties the harness relies on — no wave ever holds two
 * subtasks that declare the same file, the wave count stays minimal, the same
 * declaration always produces the same plan, and a declaration the planner
 * cannot check is reported instead of assumed harmless.
 */

import { strict as assert } from 'node:assert'
import { test } from 'node:test'

import { normalizePath, planWaves, WavePlanError } from '../src/waves.ts'
import type { Subtask } from '../src/waves.ts'

/** The plan's waves as `[wave index, subtask ids]` pairs, for compact assertions. */
function wavesOf(subtasks: readonly Subtask[]): (readonly string[])[] {
  return planWaves(subtasks).waves.map(wave => wave.subtasks)
}

test('file-disjoint subtasks share one wave', () => {
  const plan = planWaves([{ id: 'api', files: ['src/api.ts'] }, { id: 'ui', files: ['src/ui.ts'] }])
  assert.deepEqual(plan.waves.map(wave => wave.subtasks), [['api', 'ui']])
  assert.equal(plan.waves[0]?.parallel, true)
  assert.deepEqual(plan.conflicts, [])
  assert.equal(plan.depth, 1)
})

test('two subtasks that declare one file are forced into separate waves', () => {
  const plan = planWaves([
    { id: 'writer', files: ['src/store.ts'] },
    { id: 'reader', files: ['./src/STORE.ts'] },
  ])
  assert.deepEqual(plan.waves.map(wave => wave.subtasks), [['writer'], ['reader']])
  // A single-subtask wave has nothing to parallelize, and the plan says so
  // rather than asking a caller to infer it from the length.
  assert.deepEqual(plan.waves.map(wave => wave.parallel), [false, false])
  assert.deepEqual(plan.conflicts, [{ file: 'src/store.ts', subtasks: ['writer', 'reader'] }])
  assert.equal(plan.depth, 2)
})

test('a blocked subtask lands in the earliest wave it does not block', () => {
  const subtasks = [
    { id: 'a', files: ['src/a.ts'] },
    { id: 'b', files: ['src/b.ts'] },
    { id: 'c', files: ['src/a.ts'] },
    { id: 'd', files: ['src/b.ts'] },
    { id: 'e', files: ['src/c.ts'] },
  ]
  assert.deepEqual(wavesOf(subtasks), [['a', 'b', 'e'], ['c', 'd']])
})

test('a conflicting subtask pair raises the depth by one round, not one wave', () => {
  // c overlaps both a and b, so it waits for the wave holding both, and no
  // third wave appears: the earliest-fit rule keeps the plan minimal.
  const plan = planWaves([
    { id: 'a', files: ['src/a.ts'] },
    { id: 'b', files: ['src/b.ts'] },
    { id: 'c', files: ['src/a.ts', 'src/b.ts'] },
  ])
  assert.deepEqual(plan.waves.map(wave => wave.subtasks), [['a', 'b'], ['c']])
  assert.deepEqual(plan.conflicts, [
    { file: 'src/a.ts', subtasks: ['a', 'c'] },
    { file: 'src/b.ts', subtasks: ['b', 'c'] },
  ])
})

test('path spellings that name one file are one identity', () => {
  assert.equal(normalizePath('  .//src/Store.ts/  '), 'src/store.ts')
  const plan = planWaves([
    { id: 'one', files: ['src/store.ts'] },
    { id: 'two', files: [String.raw`src\store.ts`] },
    { id: 'three', files: ['./src//store.ts'] },
  ])
  assert.equal(plan.depth, 3)
  assert.deepEqual(plan.conflicts, [{ file: 'src/store.ts', subtasks: ['one', 'two', 'three'] }])
})

test('a subtask that declares no files joins the first wave and is reported', () => {
  const plan = planWaves([
    { id: 'survey', files: [] },
    { id: 'edit', files: ['src/only.ts'] },
  ])
  assert.deepEqual(plan.waves.map(wave => wave.subtasks), [['survey', 'edit']])
  assert.deepEqual(plan.undeclared, ['survey'])
})

test('the same declaration always produces the same plan', () => {
  const subtasks = [
    { id: 'b', files: ['src/b.ts'] },
    { id: 'a', files: ['src/a.ts'] },
    { id: 'c', files: ['src/a.ts'] },
  ]
  // Wave membership follows declaration order, so a re-declared list plans the
  // same waves even though the ids are not sorted.
  assert.deepEqual(planWaves(subtasks), planWaves(subtasks))
  assert.deepEqual(planWaves(subtasks).waves.map(wave => wave.subtasks), [['b', 'a'], ['c']])
})

test('a duplicate file inside one subtask is not a conflict with itself', () => {
  const plan = planWaves([{ id: 'one', files: ['src/a.ts', './src/A.ts'] }])
  assert.deepEqual(plan.conflicts, [])
  assert.equal(plan.depth, 1)
})

test('a list the planner cannot check is refused loudly', () => {
  assert.throws(() => planWaves([]), WavePlanError)
  assert.throws(() => planWaves([{ id: '  ', files: ['src/a.ts'] }]), /non-empty id/u)
  assert.throws(() => planWaves([{ id: 'a', files: [] }, { id: 'a', files: [] }]), /more than once/u)
  assert.throws(() => planWaves([{ id: 'a', files: ['  '] }]), /blank file path/u)
})
