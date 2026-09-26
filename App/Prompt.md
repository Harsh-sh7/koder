# Build prompt — AI Harness

## Context

Repo root `AI_Harness/` contains three top-level directories:

- **`WARP/`** — a vendored checkout of `warpdotdev/warp`. Read-only. Never write or
  edit anything inside this directory.
- **`Deepseek/`** — a vendored checkout of `deepseek-ai/deepseek-harness` (dsh), an
  open-source plugin-based agent runtime (Node/TS, "everything is a plugin," built on
  the Cordis micro-kernel). Read-only. Never write or edit anything inside this
  directory either.
- **`App/`** — everything we build. Every new file this project needs goes here.

**Do not add files to `WARP/` or `Deepseek/`.** Both are vendored upstream checkouts;
writing into them makes future upstream updates conflict with our own work. Reach into
them only through each ecosystem's own supported extension mechanism: dsh's plugin
system (install our packages from `App/` via `pnpm add link:../App/dsh-plugins/<pkg>`,
or a documented plugin-install path), and Cargo path dependencies for the optional
Warp layer (`App/warp_harness/Cargo.toml` depending on `../../WARP/crates/*`, never a
crate physically added inside `WARP/`).

Read `Deepseek/docs/architecture.md` and `Deepseek/AGENTS.md` before writing any
plugin code — dsh enforces real conventions (capability seams as Service
Definition/Provider/Consumer, `ctx.effect()`-based registration, strict TypeScript)
that new plugins must follow even though they live outside its repo.

## Objective — dsh is the primary chassis, everything new lives in `App/`

Build a prompt-locked, test-driven, multi-agent coding harness:

1. Freeze the incoming task prompt at session start; nothing downstream may rewrite
   the goal mid-run. (`App/dsh-plugins/spec-lock`)
2. Decompose the prompt into subtasks, minimizing which subtasks touch the same
   files — group non-conflicting subtasks into parallel waves, push
   file-overlapping subtasks into a final serial wave.
   (`App/dsh-plugins/hackathon-harness-preset`)
3. Execute each subtask through `Deepseek/packages/terminal`, `subprocess`, `shell` —
   reused as-is, no new execution code.
4. Write acceptance tests from the locked spec before a subtask's code is written,
   re-run them after.
5. Fall back to a multi-agent committee via `Deepseek/packages/subagent` (quorum vote,
   differentiated roles) whenever no test suite is runnable.
6. On failure, re-enter task composition for only the failing subtask, using
   `Deepseek/packages/guard` for the hard iteration/token cap and
   `Deepseek/packages/interaction` to escalate to a human on exhaustion.

## Hard constraints — hackathon compliance

- `Makefile` **must** live at the `AI_Harness/` repo root (not inside `App/`) — this
  is an explicit requirement in the submission guideline. `make setup && make run`
  must succeed from a clean clone.
- Credential arrives **only** via `AI_API_KEY`. Deepseek's own built-in LLM providers
  default to `DEEPSEEK_API_KEY` (confirmed in `Deepseek/AGENTS.md`) — do not use them
  as-is. Write `App/dsh-plugins/llm-harness-provider`, implementing the same Service
  Definition/Consumer capability seam as the built-in DeepSeek providers, reading
  `AI_API_KEY` exclusively. Never hardcode a credential anywhere, including inside
  `App/`.
- Text-only model I/O throughout.
- Model choice declared in `App/dsh-plugins/hackathon-harness-preset/cordis.yml`,
  overridable without touching source.
- `make run` must launch the harness and **keep it running** — the evaluation issue
  arrives as a separate step afterward (Step 5), not as a launch argument. Prefer
  `Deepseek/packages/acp`'s server mode over a one-shot CLI call for this reason.
  Verify the production (non-demo) invocation in `Deepseek/docs/architecture.md` —
  `pnpm run demo:acp` in dsh's own `package.json` is a demo script, not necessarily
  what our composition should call.
- Reproducible from a clean clone; document non-determinism in the README.

## Where to build it — all inside `App/`

```
App/
├── dsh-plugins/
│   ├── llm-harness-provider/    new — reads AI_API_KEY only
│   ├── spec-lock/                new — freezes the incoming issue
│   └── hackathon-harness-preset/ new — composed cordis.yml wiring everything together
└── warp_harness/                 optional stretch — own Cargo project, path-deps
                                   into ../../WARP/crates/*, built last if at all
```

1. **`App/dsh-plugins/llm-harness-provider/`** — the one package that can't be
   satisfied by an existing dsh plugin. Reads `AI_API_KEY` only.
2. **`App/dsh-plugins/spec-lock/`** — a `context/`-family plugin (dsh's "request-context
   plugins" pattern) freezing the issue at session start.
3. **`App/dsh-plugins/hackathon-harness-preset/cordis.yml`** — wires the two plugins
   above with `Deepseek/packages/guard` (iteration/token cap),
   `Deepseek/packages/subagent` (2-3 differentiated committee roles, quorum not
   unanimity), and the file-conflict wave-composition logic.
4. Get these installed into `Deepseek/`'s runtime via `pnpm add
   link:../App/dsh-plugins/<pkg>` run from inside `Deepseek/` — verify this resolves
   cleanly with dsh's build tooling. If it doesn't, fall back to a one-line stub
   package inside `Deepseek/packages/preset/hackathon-harness/` that only re-exports
   the real logic from `App/` — keep this the one narrow, deliberate exception to
   "never write into `Deepseek/`," and document it as such if used.
5. Follow dsh's own conventions from `Deepseek/AGENTS.md` while writing these
   packages: every contribution through `ctx.effect()`/`ctx.on()`, strict TypeScript,
   JSDoc on every export.

Only after the above works end to end, and only if time remains:

6. **`App/warp_harness/`** — its own Cargo project (own `Cargo.toml`), with path
   dependencies into `../../WARP/crates/warp_tui` and whatever else it needs. Builds a
   binary that acts as a client to `Deepseek/packages/acp`'s server, giving the
   evaluator Warp's terminal UI instead of dsh's default web UI. `WARP/` itself gains
   no new crate and no edited `Cargo.toml`. This bridge is unverified — confirm the
   ACP server's client contract before investing build time. Skip it entirely if it
   doesn't fit the remaining time; dsh's own terminal surface already satisfies the
   evaluation requirements.

## Build hygiene

- `Deepseek/` and `App/` both require Node ^22.19 or >=24 and pnpm on `PATH`.
- `App/warp_harness`, if built, should scope every `cargo` invocation to only the
  crates it path-depends on — never build all of `WARP/`'s 60+ crate workspace.

## Definition of done for this pass

- [ ] `App/dsh-plugins/llm-harness-provider` reads `AI_API_KEY` only, tested in
      isolation, never touches `DEEPSEEK_API_KEY`.
- [ ] `App/dsh-plugins/spec-lock` freezes the issue at session start; a test confirms
      mutation after freeze is rejected.
- [ ] `App/dsh-plugins/hackathon-harness-preset/cordis.yml` composes the above with
      `guard/`, `subagent/`, and the file-conflict wave logic.
- [ ] Root `AI_Harness/Makefile` (not inside `App/`) has working `setup`, `run`,
      `test`, `clean`, correctly `cd`-ing into `Deepseek/`, `App/`, and optionally
      `WARP/`/`App/warp_harness`.
- [ ] `make setup && make run` boots the harness from a clean clone using only
      `AI_API_KEY`.
- [ ] `AI_Harness/.env.example` present with `AI_API_KEY=` only — no
      `DEEPSEEK_API_KEY`, no real credential anywhere including git history.
- [ ] `App/PRD.md` and `App/ARCHITECTURE.md` reflect what was actually built.
- [ ] Nothing has been written into `WARP/` or `Deepseek/` except the documented
      link-install step (and the stub fallback, if used and clearly noted as such).
- [ ] No image/audio/video code path anywhere.

## Working order

1. `App/dsh-plugins/llm-harness-provider`, tested in isolation.
2. `App/dsh-plugins/spec-lock`.
3. `App/dsh-plugins/hackathon-harness-preset` composing provider + `guard/` + a single
   subtask running end to end through `Deepseek/`'s terminal/subprocess execution.
4. Test-first review wired onto that single subtask.
5. `subagent/`-based committee fallback for untestable subtasks.
6. File-conflict partitioner for multi-subtask parallel waves.
7. Root `Makefile` / `.env.example` / confirmed ACP entry point.
8. Only if time remains: `App/warp_harness/`.
