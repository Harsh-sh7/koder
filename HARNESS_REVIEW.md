# Koder AI Harness — Review, Fixes & Roadmap

27 Sep 2026

## Verdict

Koder is now eval-ready from a clean clone: `make setup && make run` with only
`AI_API_KEY` opens a session on OpenRouter's DeepSeek and runs an issue end to
end. Before this review it could not open a session at all from a fresh clone.

- **Architecture is strong for the presentation round.** An unmodified upstream
  engine (dsh) with all behaviour mounted as three plugins, a frozen task spec,
  a deterministic wave partitioner, a review committee, a harness-enforced
  budget, and a native Rust app over ACP.
- **The scoring round was at risk from configuration, not design.** Eight
  eval-critical defects are fixed below.
- **Per-request overhead fell 40%** (27,673 → 16,687 characters of prompt
  prefix, 28 → 16 tools), which compounds over ~40 issues × 20–40 steps.
- **Not verified:** a run against a real DeepSeek or Qwen key. Every test ran
  against a local recording model server. The first real-key run is item 1 of
  the eval-day checklist.

## Eval-critical bugs fixed

| # | Defect | Impact on the eval | Fix | Evidence |
| --- | --- | --- | --- | --- |
| 1 | `make setup` only ran `pnpm install` on dsh. A session needs dsh's native addon and `lib/typert.host.js` bundles, which only dsh's build produces | Engine could not open a session from a clean clone: every issue scores zero | Setup runs `build:native-system`, `tsc -b tsconfig.host.json`, `tsdown` (host face) | Fresh-clone rehearsal: clone + setup + session in 2 min 14 s |
| 2 | Launcher overlay restated the provider row with only `modelsFile`; a Cordis override replaces an entry's whole config | The `AI_HARNESS_FALLBACK_*` route was silently dropped | Overlay restates the full config | Request reached the recording server with the chosen model |
| 3 | No default model; `.env` model settings never reached the composition | Evaluator had to know hidden variables or every prompt fails | Default `deepseek/deepseek-v4-flash` on OpenRouter; `AI_MODEL`, `AI_BASE_URL`, `AI_CONTEXT_WINDOW`, `AI_TEMPERATURE` from env or `.env` | `AI_MODEL=qwen/qwen3.7-plus` in `.env` produced a qwen request |
| 4 | Budget counted prompt-cache reads at full weight against a 400k **per-session** cap | A normal issue stopped after ~15 steps; 40 issues in one session shared 400k | Per-prompt scope, cache reads × 0.1, cap 1.5M weighted, 90 steps | 5 new tests |
| 5 | Step-cap stop + approved escalation raised the *token* ceiling | Each grant bought exactly one step | Step-cap grant adds half the step cap | Test: steps 5–6 admitted, 7 refused |
| 6 | Spec-lock froze the first user message forever | Issues 2–40 in one session were held to issue 1; compaction re-injected issue 1 | A later prompt ≥ 80 chars re-freezes; short follow-ups don't | 2 new tests |
| 7 | 28 tools per request incl. plan mode (waits for approval), goal rounds, web search (needs DeepSeek key), `read_image` (breaks text-only I/O) | ~3k tokens/step wasted + stall risks | 12 plugin rows mounted off in `cordis.yml` | Prefix 27,673 → 16,687 chars; clean boot |
| 8 | Committee reviewers inherited `edit`/`write` | A reviewer could change the code under review | Default reviewer mask denies write + delegation tools | Typecheck + tests |

Also fixed: Makefile broke on paths containing spaces; `make test` failed on a
clean clone (now uses dsh's tsconfig path facade); the operating contract gained
a 10-step issue procedure (locate, reproduce, minimal root-cause fix, never
weaken tests, verify nearest tests, clean scratch files, report). Stale doc
claims in README, context.md, takeover.md and Architecture.md were updated.

## UI bugs fixed

The window works at any width from 640 pt, and hover/clicks land on what they
point at. Checked with the app's own screenshot mode at 1480, 1100, 900, 700 pt.

| Bug | Root cause | Fix |
| --- | --- | --- |
| Hover blinked off; clicks on row text did nothing (sidebar, session panel, suggestions, dialogs) | egui labels are selectable by default and sat on top of the row's click area | `theme::list_row` makes its labels non-selectable |
| Not responsive: at 900 pt the card was ~360 pt, ~275 pt with the tree open | Sidebar 236 / panel 312 were constants | `theme::columns(width)`: icon-rail sidebar under 1040 pt; panel floats over the card (above the composer) when the card would drop under 520 pt; tree ≤ 28% of width |
| Help button sat beside Send when the panel was closed | Fixed corner position | Moves into the footer strip when the panel isn't docked |
| Chevrons by "Auto approve" and the model name did nothing | Hover-only sense | Now clickable |
| ⌘P opened the command palette (key card says "go to file") | Both keys → one action | ⌘P opens go-to-file |
| Esc never reached the terminal (vim, less) | Always consumed | Consumed only when something is open |
| ⌘W discarded unsaved edits | No dirty check | Refuses with a toast until saved |
| Dialogs opened flush with the window top | Positioned from first-frame rect | Anchored top-centre |
| Profile name ran under the palette/gear icons | Name laid out first | Name reserves room |

New tooling: `--size WxH`. Window floor 900×560 → 640×480. Three unit tests pin
the column rules.

## How subagents divide the work today

The model, not the harness, decides how an issue is split. One turn:

1. Spec-lock freezes the issue text (now per new issue).
2. The model decides how many parts. One part: locate → reproduce → fix → verify.
3. Two or more: `wave_plan` groups parts with no shared file into waves; wave
   members may run as `subagent` children.
4. Checks run. Pass → report. Fail → redo that part. No runnable check →
   `committee` (3 reviewers, quorum).
5. Budget guard before every step: warn at 80%, stop at 1.5M weighted tokens or
   90 steps per issue.

Where it is weaker than it looks:

- **Division:** `wave_plan` is earliest-fit arithmetic on *declared* files.
  Shared imports, lockfiles, full test runs are invisible to it.
- **Parallelism:** wave members edit the same working tree — no worktrees, no
  merge step; wave order is advisory.
- **Child context:** children start fresh; passing the spec/files/check is a
  contract rule, not enforced.
- **Committee:** 3 fresh reviewers each re-explore the repo (~3 small agent
  runs). Now only when no runnable check exists; reviewers can't edit.
- **Test depth: shallow.** No test-command detection, no baseline run, no
  edge-case/hidden-test generation. Depth depends on the model.

## Still flagged (not changed)

| Risk | Why it matters | Recommendation |
| --- | --- | --- |
| Sandbox is `workspace-write`: writes outside the repo (global pip/npm caches, `~/.cache`) are denied and need an approval the evaluator's client may not answer | Installs or some test runners fail | Measure on practice repos; if hit, set `DSH_PERMISSION_MODE=danger-full-access` for the eval run, or use a repo-local venv |
| No real-key run yet | Tool-call quirks of DeepSeek/Qwen via OpenRouter unproven | Run 3–5 practice issues on each model before submission |
| Temperature unset by default | Provider defaults (often 1.0) add variance | Try `AI_TEMPERATURE` 0.0–0.3 (DeepSeek) and ~0.7 (Qwen coder guidance); keep what scores best |
| Setup is heavy (~2 min, ~2 GB, C compiler needed) | Evaluator machine must have Xcode CLT / cc | State it in README (done) |
| dsh pinned to a developer-preview rc | Upstream breaking changes | Keep the pin |
| Sidebar labels (Knowledge Center → Workflows, Sites → Editor, Extensions → Models) | Judges may read them as mislabelled | Rename to what they open, or add tooltips |
| No per-issue telemetry file | "Evidence over claims" is scored | See roadmap item 3 |

## New features: uniqueness, efficiency, reliability

Ordered by expected score gain per hour.

| # | Feature | What it does | Why it wins | Effort |
| --- | --- | --- | --- | --- |
| 1 | **Repo card** (zero-token recon) | On session start the harness (not the model) detects language, test command, layout; runs the suite once; injects a ≤ 300-token card incl. tests already failing | Removes 3–8 exploration steps per issue; lets the model tell regressions from pre-existing failures | 3–4 h |
| 2 | **Verify gate before end_turn** | When the model tries to finish, the harness runs the repro + nearest tests itself; red → injects the failure and continues (bounded by budget) | "Evidence over claims" enforced by code, not prompt — the harness's signature feature | 3 h |
| 3 | **Per-issue evidence report** | Writes `.harness/runs/<issue>.json`: tokens (uncached/cached/output), API calls, steps, wall time, files changed, check commands + exit codes | Hard numbers for the presentation and for tuning | 2 h |
| 4 | **Hidden-test hunter** | One cheap child writes 3–5 edge-case tests (empty, boundary, error path, unicode) from the issue text before the fix | Mirrors how graders test; catches partial fixes | 3 h |
| 5 | **Diff-scope guard** | After edits, git diff: flags changed test files, lockfiles, unrelated modules, leftover scratch files | Prevents "made the test pass by editing the test" and stray files that break grading | 2 h |
| 6 | **Diff-only reviewer** | Replace the 3-agent committee default with one tool-less call on diff + test output | ~10× cheaper review; keep the committee for design calls | 1–2 h |
| 7 | **Worktree waves** | Each parallel subagent gets a git worktree; harness merges + re-runs tests | Makes the parallel-waves claim real and safe | 5–6 h |
| 8 | **Model profiles** | `AI_MODEL` prefix selects temperature, context window, tool-result limits per family (DeepSeek vs Qwen) | Same harness tuned to both judged models | 1–2 h |
| 9 | **Tighter tool-result pruning for test output** | Keep failing test names + first trace, drop passing noise | Test logs are the biggest context sink | 1 h |

Best presentation story: items 1 + 2 + 3 — "the harness computes what the model
would rediscover, refuses unproven completions, and reports the cost of every
fix."

## Eval-day checklist and how this was verified

- [ ] Real-key run on 3–5 practice issues with `AI_MODEL` = DeepSeek, then Qwen
- [ ] Decide `AI_TEMPERATURE` per model from those runs
- [ ] Check whether any practice repo needs writes outside the workspace (sandbox)
- [ ] Fresh clone → `cp .env.example .env` → key → `make setup` → `make run`
- [ ] Commit and push (nothing has been committed)

Verified in this review:

- `make test` 58/58 plugin tests; `make typecheck` clean
- `cargo test --workspace`: 66 core + 7 app + 1 terminal smoke
- `make check-native` 14 passed; `make check-engine` 16 passed, session opens on
  `deepseek/deepseek-v4-flash`
- Fresh-clone rehearsal in a path with spaces: clone + setup + real session, 2 min 14 s
- Engine sessions against a recording server: model, tool list, prefix size and
  temperature read off the actual request
- UI screenshots at 1480, 1100, 900, 700 pt

## Update — subagents, history, and the UI (second pass)

| Area | Before | Now | Evidence |
| --- | --- | --- | --- |
| Subagents | The model *could* call `subagent`; small models never did, so every job ran serially in one context | New `run_wave` tool: one call partitions the parts into file-disjoint waves and runs each wave's parts as **parallel** children; each child may write only its declared files (enforced in `tools/pre-execute`, not just prompted) and reports through a structured result. The parent pays one round trip for the whole job | 6 new tests (parallel start, wave ordering, results, ownership guard); live run with the mock: three children interleaved, each wrote its file |
| Contract | "may run together via subagent" | Rule 7: two or more parts with different files → `run_wave`; a single part or a one-place fix → do it yourself (no token-wasting delegation) | Preset tests |
| History | `new_task` discarded the conversation; nothing on disk | Every task saved to `.harness/tasks/<id>.json` (atomic writes, every 5 s while running, at turn end, on quit); listed per workspace; reopen restores the transcript and **resumes the engine session** | 3 core tests + an app test; resume proven across two engine processes |
| Stop | No stop anywhere; `session/cancel` was sent as a request, which the engine ignores | Stop in the composer, the activity card and each running subagent; cancel is a notification | Protocol fix in `acp.rs` |
| Stray updates | Events from any session drew into the task on screen | Updates are filtered by session id | `AgentEvent::session()` |
| Engine start | Refused to start without `.harness/models.json` → "no model route is configured" | Starts on the default route | — |
| Chat rendering | Word-by-word labels; no tables, numbered lists, quotes; user bubble overflowed left and dragged later rows off-card; code scroll ids collided | `egui_commonmark` for prose; own verbatim code/diagram blocks (copy, fold, sideways scroll, unique ids); measured, wrapped user bubbles; readable tool rows ("Ran `cargo test`", "Read src/x.rs"); file cards with Open; error card carries its reason once; jump-to-latest button; welcome screen | Screenshots at 1440, 1300, 1200 and 760 wide |
| Layout | Fixed widths; window could open taller than the screen | Reference-style chrome (traffic lights in the sidebar, own drag strip), animated sidebar retract/expand, back/forward, rail below 1040 pt, floating panel below the header, window fitted to the monitor on launch | Screenshots |
| Panel | Subagents + processes | Subagents (with state and stop), Skills & MCP (real skill roots, `.harness/mcp.json` servers passed to sessions), Artifact (files created/changed, incl. wave outputs), Processes | Screenshots |
