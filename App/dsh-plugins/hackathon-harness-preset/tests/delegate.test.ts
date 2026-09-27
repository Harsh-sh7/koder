/**
 * `run_wave` hands a job's parts to parallel children, so the pure half of it
 * is the contract a child works under: what it is told, which writes it may
 * make, and what the parent reads back. The composed tool itself is exercised
 * in `preset.test.ts` against a fake subagent service.
 */

import { strict as assert } from 'node:assert'
import { test } from 'node:test'

import { ownsPath, partPrompt, renderRun } from '../src/delegate.ts'

const cwd = '/work/repo'

test('a part owns exactly its declared files, however the path is spelled', () => {
  assert.ok(ownsPath(['src/todo.py'], 'src/todo.py', cwd))
  assert.ok(ownsPath(['src/todo.py'], './src/todo.py', cwd))
  assert.ok(ownsPath(['src/todo.py'], '/work/repo/src/todo.py', cwd))
  assert.ok(ownsPath(['SRC/Todo.py'], 'src/todo.py', cwd), 'case folds, as the planner does')
  assert.ok(ownsPath(['docs/'], 'docs/guide/intro.md', cwd), 'a declared directory owns what is under it')
  assert.ok(!ownsPath(['src/todo.py'], 'tests/test_todo.py', cwd))
  assert.ok(!ownsPath(['src/todo.py'], 'src/todo.pyc', cwd), 'a prefix of the name is not the file')
  assert.ok(!ownsPath(['src/todo.py'], '../other/src/todo.py', cwd), 'nothing outside the workspace')
  assert.ok(!ownsPath([], 'README.md', cwd), 'a part with no files owns none')
})

test('a child is told its part, its files, its siblings and its check — and nothing it cannot use', () => {
  const prompt = partPrompt(
    { id: 'tests', task: 'Write pytest tests for add and list.', files: ['tests/test_todo.py'], check: 'pytest -q' },
    'Build a todo CLI.',
    ['core', 'docs'],
  )
  assert.match(prompt, /The overall job .*\nBuild a todo CLI\./u)
  assert.match(prompt, /Your part, "tests": Write pytest tests for add and list\./u)
  assert.match(prompt, /Files you own: tests\/test_todo\.py\. Write only these/u)
  assert.match(prompt, /Other agents are working on core, docs at the same time/u)
  assert.match(prompt, /`pytest -q` must pass/u)
})

test('a long spec reaches a fresh child in full, and a part without a check is told how to prove itself', () => {
  const prompt = partPrompt({ id: 'a', task: 'Do a.', files: [] }, 'x'.repeat(2000), [])
  assert.doesNotMatch(prompt, /call spec_get/u)
  assert.ok(prompt.includes('x'.repeat(2000)))
  assert.match(prompt, /owns no file/u)
  assert.match(prompt, /smallest command that exercises your part/u)
  assert.doesNotMatch(prompt, /Other agents/u)
})

test('the parent reads one line per part and the verification it still owes', () => {
  const text = renderRun([
    { id: 'core', wave: 1, status: 'done', summary: 'todo.py added; pytest passed.' },
    { id: 'docs', wave: 1, status: 'failed', summary: 'README blocked on the CLI name.' },
  ], 1)
  assert.match(text, /^1 of 2 parts done in 1 wave\(s\):/u)
  assert.match(text, /\[wave 1\] core: done — todo\.py added; pytest passed\./u)
  assert.match(text, /\[wave 1\] docs: failed — README blocked/u)
  assert.match(text, /Fix the failed parts yourself/u)
})
