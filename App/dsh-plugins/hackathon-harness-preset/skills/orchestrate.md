# Orchestrating a job in this harness

Load this when a job is bigger than one edit: several files, several parts, or a
part you cannot prove. The operating contract in your prompt is the short form;
this is the procedure behind it.

## 1. Freeze what you are building

The session's frozen spec is the job, and it is also the acceptance test. Read
it once, then write down what "done" means in terms of observable behaviour: the
commands that must pass, the files that must exist, the output that must appear.

If the spec is ambiguous, ask the user — do not choose silently and do not build
both alternatives. An ambiguity you resolve yourself is a guess the user has to
review later anyway.

## 2. Decompose into subtasks that name their files

A subtask is a unit of work one agent can finish and one check can prove. Good
subtasks are small enough to verify alone and large enough to be worth a handoff.

For each subtask, write down the files it will write. This is the input to
`wave_plan`, and the declaration is what the harness trusts: a subtask whose
files you cannot name is not yet decomposed — explore until you can, or fold it
into the subtask that owns those files.

Call `wave_plan` with `{id, files}` for every subtask:

- Waves come back ordered. Work them in order.
- Members of one wave share no file, so they may be delegated together.
- `conflicts` names the files behind every forced ordering — those pairs must
  never run at the same time, whatever else you parallelize.
- `undeclared` lists subtasks whose files you did not name. Those are reported,
  not blessed: if two of them touch the same file, the plan could not see it.

## 3. Prove before you build

For each subtask, the check comes first:

- A test in the repo's own framework, when there is one.
- A command whose output shows the new behaviour, when there is not.
- A diff assertion (exact expected content) for generated or configuration files.

Run the check before the implementation and watch it fail — a check that passes
before the change proves nothing about the change. Keep the command: you will
run it again, and so will the reviewer.

A subtask you cannot check is not done, only written.

## 4. Run the waves

Inside one wave, delegate the members together when the work is substantial, and
do the smallest ones yourself. Give every delegate:

- The subtask, in its own words, with the files it owns.
- The frozen spec text (or the `spec_get` digest) — delegates start fresh and
  cannot see your conversation.
- The check it must make pass, named exactly.
- What it must return: the files changed and the check's output.

A delegate that reports success without the check's output has reported nothing.
Run the check yourself before accepting the subtask.

## 5. Review what no check can prove

Some work has no runnable check: a design decision, an API shape, prose, a
refactor whose behaviour is meant to be identical. Before reporting such a
subtask done, convene `committee` on the one decision that matters — not on the
whole change:

- State the decision as a claim the reviewer can falsify.
- Give evidence as paths and commands, not pasted output: reviewers share the
  workspace and can read a file far more cheaply than you can quote it.
- Act on the quorum verdict. A rejected verdict is a finding: fix the subtask,
  or change the decision and re-convene. If you believe the reviewer is wrong,
  say so in your report with the reason — do not silently overrule a quorum.

## 6. On failure, re-enter composition for that subtask only

A failed check means the subtask's plan was wrong, not the whole job:

1. Re-read the check's output and the spec before changing anything.
2. Re-compose just that subtask — new files, new approach, possibly a smaller
   split — and re-plan only the waves it belongs to.
3. Leave the subtasks whose checks passed exactly as they are. Re-running proven
   work is the most expensive way to lose a budget.

## 7. Land the budget

The harness counts model tokens and steps itself and stops the turn when either
runs out. You do not need to track it, but you do need to react to the warning:

- Stop starting new subtasks. Finish the one in flight.
- Report: what passed, what did not, and the exact command that shows each.
- If the budget is exhausted mid-turn, the harness asks the user before spending
  more, so a report that ends in a clean state is what makes that ask answerable.

Never spend the last of the budget on unverified work: a subtask reported as
done with a passing check is worth more than three attempts without one.
