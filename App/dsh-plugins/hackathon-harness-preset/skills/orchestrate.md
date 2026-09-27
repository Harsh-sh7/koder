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

## 2. For a new project or a substantial new feature, design it before you build it

If the job is starting something new — an app, a service, a feature that adds
several files — write the architecture down before creating anything:

- the stack (language, framework, storage) and why, when the spec left it open
- the folder layout: what lives where, and why
- the modules or components and what each one owns
- how data moves through it (a request in, a response out; a store and who
  reads and writes it)

A few lines in your reply are enough for a small app; write it to `PLAN.md` when
the project is large enough that a later session (or a subagent) needs to read
it back. This is what separates a project that reads as intentionally organised
from one that reads as generated. Skip this step for a fix inside an existing
project: its structure is already decided.

Then do not create those files yourself. The plan you just wrote already named
more than one file — an implementation and its tests, at minimum — so go
straight to step 3 and hand every one of them to `run_wave` in a single call.
This holds even for a task that reads as "simple": a todo app with tests and a
README is three parts (core, tests, docs), and treating it as one part you
write serially yourself is exactly the mistake this step exists to prevent.

## 3. Decompose into subtasks that name their files

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

## 4. Prove before you build

For each subtask, the check comes first:

- A test in the repo's own framework, when there is one.
- A command whose output shows the new behaviour, when there is not.
- A diff assertion (exact expected content) for generated or configuration files.

Run the check before the implementation and watch it fail — a check that passes
before the change proves nothing about the change. Keep the command: you will
run it again, and so will the reviewer.

A subtask you cannot check is not done, only written.

## 5. Run the waves

Call `run_wave` once with every part — its task, the files it owns, and its
check. The harness partitions the parts into file-disjoint waves itself (the
same arithmetic as `wave_plan`), runs each wave's parts as parallel subagents,
holds every part's agent to its own files, and returns one line per part.

- Write each task for an agent that has never seen this conversation.
- Name the check exactly; each part's agent must make it pass.
- Do the smallest parts yourself instead: a part is worth a handoff only when
  it is real work.

A part that reports `done` without its check passing has reported nothing. Run
the checks yourself — one combined command is cheapest — before accepting.

## 6. Review what no check can prove

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

## 7. On failure, re-enter composition for that subtask only

A failed check means the subtask's plan was wrong, not the whole job:

1. Re-read the check's output and the spec before changing anything.
2. Re-compose just that subtask — new files, new approach, possibly a smaller
   split — and re-plan only the waves it belongs to.
3. Leave the subtasks whose checks passed exactly as they are. Re-running proven
   work is the most expensive way to lose a budget.

## 8. Land the budget

The harness counts model tokens and steps itself and stops the turn when either
runs out. You do not need to track it, but you do need to react to the warning:

- Stop starting new subtasks. Finish the one in flight.
- Report: what passed, what did not, and the exact command that shows each.
- If the budget is exhausted mid-turn, the harness asks the user before spending
  more, so a report that ends in a clean state is what makes that ask answerable.

Never spend the last of the budget on unverified work: a subtask reported as
done with a passing check is worth more than three attempts without one.

Before reporting a multi-part job done, call `harness_report`. It reads the same
budget and `run_wave` state the harness already tracks — spend against the
ceiling, steps against the cap, escalations used, and every part's outcome — and
turns it into a short findings list. It costs nothing to call and it never
changes anything; use it as the last check before you say the job is finished,
the way you would run the checks one more time.
