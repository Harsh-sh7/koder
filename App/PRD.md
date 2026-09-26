# PRD — AI Harness

**Status:** Draft — update as implementation proceeds
**Owners:** [team names]
**Event:** AI Harness Hackathon 2026
**Repo layout:** `AI_Harness/` root, with `WARP/`, `Deepseek/` (vendored, read-only),
and `App/` (everything we build) as siblings.

## 1. Problem statement

Autonomous coding agents fail in two predictable ways: they drift from the original
task as they iterate, and they either burn excessive tokens re-reading messy context
or produce code that looks plausible but hasn't actually been checked against
anything. We fix both by (a) freezing the task up front so the agent can't quietly
redefine it, and (b) making every iteration terminate in a real pass/fail signal —
automated tests or a reviewing committee — rather than the agent grading its own
homework. We build this on `Deepseek/` (deepseek-ai/deepseek-harness), whose plugin
architecture already provides most of the mechanism, adding only what it doesn't have
— all of it kept inside `App/`, never written into either vendored checkout.

## 2. Goals

- Given a locked prompt (a GitHub issue), produce working code that passes the
  associated test suite, or a structured failure report if it can't within budget.
- Reuse `Deepseek/packages/guard`, `subagent`, `session`, `terminal`/`subprocess`/
  `shell`, and `interaction` rather than reimplementing their mechanics.
- Add only the genuinely missing pieces — `App/dsh-plugins/llm-harness-provider`
  (AI_API_KEY), `App/dsh-plugins/spec-lock` (prompt freezing), and file-overlap-aware
  wave composition — with zero files written into `WARP/` or `Deepseek/`.
- Comply fully with the hackathon's Makefile / credential / text-only-model
  requirements, including the Makefile's required location at the `AI_Harness/` root.
- Present the harness through our own native application, `App/native/` — a Rust
  core (PTY terminal with blocks, git, files, the ACP client) behind an egui
  interface. Built, not a stretch: the ACP bridge it depends on is proven.

## 3. Non-goals

- Not reimplementing loop-guard, sub-agent delegation, or trajectory logging.
- Not multimodal.
- Not reusing Warp's implementation or source: its TUI is proprietary and not
  separable from the GUI app, so the application's renderer and visual system are
  our own. `WARP/` stays vendored, read-only, and unconsulted by the build.
- Not using `Deepseek/`'s stock DeepSeek-branded LLM providers or its `headless`
  preset as-is (both default to `DEEPSEEK_API_KEY`).
- Not editing `WARP/` or `Deepseek/` — zero exceptions, and none proved necessary:
  the profile's `link:` dependencies resolve our plugin packages by absolute path,
  so no stub is needed inside the vendored tree.

## 4. Users

- **Hackathon evaluator** — clones `AI_Harness/`, runs `make setup && make run` from
  the repo root, supplies the issue/test case to the running harness, reads the
  result.
- **Our team** — builds and iterates locally with the same root Makefile the
  evaluator uses.

## 5. Functional requirements

| # | Requirement | Maps to |
|---|---|---|
| F1 | Freeze the incoming prompt/issue at session start; no downstream mutation | `App/dsh-plugins/spec-lock` |
| F2 | Decompose the prompt into subtasks; parallel waves for non-overlapping files, a final serial wave for overlapping ones | `App/dsh-plugins/hackathon-harness-preset` |
| F3 | Execute each subtask via real shell commands | `Deepseek/packages/terminal`, `subprocess`, `shell` (reused as-is) |
| F4 | Generate acceptance tests from the locked spec before coding; re-run after | new, wired into the preset |
| F5 | Quorum-voting committee fallback when no test suite is runnable | `Deepseek/packages/subagent` (reused, configured) |
| F6 | On failure, re-enter task composition for the failing subtask only | `App/dsh-plugins/hackathon-harness-preset` + `Deepseek/packages/guard` |
| F7 | Hard-stop on budget exhaustion; escalate to a human | `Deepseek/packages/guard` + `interaction` (reused) |
| F8 | Read model credential only from `AI_API_KEY`, never `DEEPSEEK_API_KEY` | `App/dsh-plugins/llm-harness-provider` |
| F9 | Root `Makefile` (`AI_Harness/Makefile`, not inside `App/`) with `setup`/`run`/`test`/`clean` | `Makefile` |
| F10 | Text-only model I/O throughout | `App/dsh-plugins/llm-harness-provider` |
| F11 | Persistent trajectory of every message/tool call/reasoning step | `Deepseek/packages/session` (reused as-is) |
| F12 | Nothing written into `WARP/` or `Deepseek/` except the documented link-install step | repo hygiene |
| F13 | Present the harness as a native terminal-first application: terminal blocks, agent/tools/waves, editor + diff, git, observability | `App/native/` (Rust: `harness-core` + egui `harness-app`), an ACP client of `Deepseek/packages/acp` |

## 6. Non-functional requirements

- **Token efficiency:** rely on `Deepseek/packages/session`'s trajectory log rather
  than re-inventing summarization/recycling logic.
- **Reproducibility:** document temperature/seed settings; consistent behavior under
  the same evaluation conditions.
- **Security:** zero hardcoded credentials anywhere; `.env.example` lists only
  `AI_API_KEY`.
- **Portability:** clean-clone build with only the documented setup steps and stated
  Node/pnpm engine range.
- **Vendoring hygiene:** `WARP/` and `Deepseek/` stay clean upstream checkouts that
  can be updated (`git pull`) without touching our own work in `App/`.

## 7. Success metrics

- Task pass rate on the evaluation issue (primary).
- Iterations-to-pass.
- Wall-clock time from `make run` to final result.
- Token/cost spend per successful run.

## 8. Compliance checklist (from `AI_Harness_Submission.pdf`)

- [x] `Makefile` at the `AI_Harness/` repo root (confirmed: must be root, not `App/`).
- [x] `make setup && make run` succeeds from a clean environment.
- [x] No hardcoded API keys/tokens/secrets anywhere; `AI_API_KEY` read at runtime
      only, never `DEEPSEEK_API_KEY` for the harness's own model calls.
- [x] `.env.example` present at root, no real credentials.
- [x] Text-only model, no multimodal input path.
- [x] Model configuration declared in one file
      (`App/dsh-plugins/hackathon-harness-preset/cordis.yml`), overridable without
      source changes.
- [x] `make run` launches the harness and keeps it running — issue supplied
      afterward, not as a launch argument.
- [x] Non-determinism documented in the README.

## 9. Milestones

All complete — each has code and a test that covers it; `make check` is the gate.

1. `App/dsh-plugins/llm-harness-provider`, tested in isolation.
2. `App/dsh-plugins/spec-lock`.
3. `App/dsh-plugins/hackathon-harness-preset` composing provider + `guard/` +
   the execution plugins end to end.
4. Test-first review wired into the operating contract.
5. `subagent/`-based committee fallback for untestable subtasks.
6. File-conflict partitioner for multi-subtask parallel waves.
7. Root `Makefile` / `.env.example` / confirmed ACP entry point.
8. `App/native/crates/harness-core` — PTY/block terminal, git, files, search,
   the ACP client, the agent session model, token accounting.
9. `App/native/crates/harness-app` — the egui interface over it, with the
   `--check` self-test harness.

## 10. Risks

Resolved during implementation:

- ~~Stock providers default to `DEEPSEEK_API_KEY`~~ → they are mounted
  `disabled: true`; the mounted provider reads `AI_API_KEY` only (F8 is enforced by
  the composition, not by convention).
- ~~The `link:` install mechanism is unverified~~ → `init-profile.mjs` writes the
  profile's `link:` dependencies at setup time and `make setup` installs them; no
  stub inside `Deepseek/` was needed.
- ~~The ACP server's invocation is unconfirmed~~ → the profile composes dsh's own
  ACP bundle as its application, and both `App/probe/acp-smoke.mjs` and the Rust
  `acp_smoke` test drive it end to end.
- ~~The Warp bridge is unverified~~ → no Warp code is used at all.

Still true, and handled by design:

- **Non-determinism** — model behaviour varies; the caps, waves, and quorum bound
  it. Documented for the evaluator, and the mock model server keeps the tests
  reproducible.
- **Two-language project** (TypeScript plugins + a required Rust toolchain) — the
  setup check states the required versions up front and fails with a message that
  names the missing tool.
- **File-conflict bugs** in parallel waves — the partitioner falls back to serial
  execution when it cannot establish non-overlap, rather than guessing.
- **Apply the same "is this genuinely swappable" skepticism** to any future
  third-party package: both `WARP/app/ai/` and `Deepseek/`'s stock providers turned
  out to be coupled to their maker's own hosted backend.
