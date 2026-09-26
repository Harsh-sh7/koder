# DeepSeek Harness (`dsh`) — the engine

Reference for the upstream project this harness runs on. The checkout was
**removed from this repository to keep it small** (it is ~2.2 GB installed); it
is gitignored and restorable in one command. Nothing of ours is ever written
into it.

> **Restore it before anything else:**
>
> ```sh
> git clone --depth 1 --branch dsh-v0.1.7-rc.2 \
>   https://github.com/deepseek-ai/deepseek-harness.git Deepseek/deepseek-harness
> cd Deepseek/deepseek-harness && pnpm install      # or: make setup, from the repo root
> ```

| Fact | Value |
|---|---|
| Upstream | <https://github.com/deepseek-ai/deepseek-harness> |
| Checkout path (required by this repo) | `Deepseek/deepseek-harness` |
| Ref this project was built against | tag **`dsh-v0.1.7-rc.2`**, commit `477b4f4205` |
| Root package | `@deepseek-ai/dsh-root` 0.1.7-rc.2 |
| License | MIT |
| Docs | <https://deepseek-harness.github.io/deepseek-harness/> |
| Runtime | Node `^22.19.0 || >=24.0.0`, pnpm 11.7.0 (declared in `package.json`) |
| Architecture | everything-is-a-plugin, on [Cordis](https://github.com/cordiverse/cordis) |
| Status | *developer preview* — "THERE WILL BE COMPATIBILITY-BREAKING CHANGES" |

Docs to read **inside the checkout**: `README.md` (run paths: `npx
@deepseek-ai/dsh web`, or from source), `AGENTS.md` and `CLAUDE.md` (the
project's own rules for agents working on *it*), `CONTRIBUTING.md`, `SAFETY.md`,
`docs/` (the user guide and the plugin/architecture docs), and `packages/*/*`
(one directory per plugin — the real documentation of the seam).

---

## 1. Why it is the right engine for this harness

- **Agent loop, tools, sessions, providers, compaction, approval, skills,
  subagents, token accounting** all exist and are battle-tested. Rewriting any of
  them would have cost the hackathon's whole budget for no product value.
- **Everything is a plugin**, so *our* behaviour (route ownership, the frozen
  spec, the operating contract, waves, the committee, the budget cap) mounts as
  bundles instead of patching source. That is what keeps the checkout pristine
  and the composition overridable.
- **It speaks ACP** (Agent Client Protocol) — a documented JSON-RPC surface over
  stdio — which is what lets a native application own the window instead of
  embedding a terminal UI.

## 2. How this repo runs it

```
node --import tsx apps/cli/src/bin.ts        # the engine, from source, via tsx
  with DSH_HOME=<repo>/.dsh                  # the profile home
  and cwd = the repository root              # so the .env layer is the checkout
```

- The **profile** is `hackathon-harness`, materialized by
  `App/scripts/init-profile.mjs` into `$DSH_HOME` (default `<repo>/.dsh`). A
  profile is a dsh package project of its own: it declares the bundles it
  composes, its `link:` dependencies (our plugins), and two scaffold files.
- The bundles, in application order:

  | Bundle | What it brings |
  |---|---|
  | `@deepseek-ai/dsh-base` | the agent, sessions, tools, providers, skills, compaction |
  | `@deepseek-ai/dsh-acp-app` | the **application**: the ACP bridge on stdin/stdout |
  | `@harness/dsh-hackathon-harness-preset` | **ours** — the overlay that turns the shipped routes off and mounts our provider, spec lock, contract, waves, committee, and budget |

- There is **no `--acp` flag**. The ACP bridge *is* the profile's application, so
  "run as an ACP server" is a property of the composition. Its app reads stdin
  until EOF and then shuts the process down.
- **Stdout is protocol.** Every diagnostic goes to stderr.

## 3. The ACP surface this app actually uses

Implemented in `App/native/crates/harness-core/src/acp.rs`; the session model
that reduces the events is `agent.rs`.

| Direction | Method / event | Used for |
|---|---|---|
| app → engine | `initialize` | handshake, capability exchange |
| app → engine | `session/new` (with `cwd`) | open a session **on a folder** — the app's workspace switch is this |
| app → engine | `session/prompt` | send the user's message — the issue arrives *after* launch |
| app → engine | `session/set_config_option` | select the model route the workspace declares |
| app → engine | `session/cancel` | stop a turn |
| app → engine | `session/request_permission` response | answer the approval gate (allow once / always / deny) |
| engine → app | `session/update` notifications | agent message chunks, thinking, tool calls and their output, plan, to-dos, usage |
| engine → app | request/response failures | **end the run** with a translated message (`state.rs::describe_failure`) |
| engine → app | engine exit | fail the conversation, drop the client |

The app treats the engine as a *process it owns*: one engine per workspace, kept
across `new_task` (a session is cheap, a process is not) and deliberately
restarted on a folder switch, because the route registry is read at launch.

## 4. The seam: how our code hooks in (never by editing upstream)

A dsh plugin is a Cordis plugin: it exports a function that receives a context
and contributes through named services. Our three packages live in
`App/dsh-plugins/`, and the preset's `cordis.yml` is where the composition is
declared as data.

| Package | Seam it uses | What it does |
|---|---|---|
| `llm-harness-provider` | the LLM provider service (`dsh-llm`, `dsh-llm-pi-ai`) | registers **every** route the app can use, from `.harness/models.json`, reading the key from `AI_API_KEY`; re-reads the document when its bytes change; serves a placeholder route so an unconfigured workspace can still open a session |
| `spec-lock` | `dsh-system-prompt`, `dsh-session-projection`, `dsh-tools`, `dsh-user-approval` | freezes the session's opening instruction: restated whenever context management drops it, amendable only with approval, readable in full via `spec_get` while a long spec travels as one pointer line |
| `hackathon-harness-preset` | `dsh-agent`, `dsh-session`, `dsh-tools`, `dsh-subagent`, `dsh-token-meter`, `dsh-user-approval`, `dsh-skill` | the operating contract in the system prompt, the `wave_plan` and `committee` tools, the budget cap with warnings/escalations, the per-turn step cap, committee reviewers seated as **spawned children**, and the `orchestrate` skill |

`cordis.yml` also **mounts off** the shipped routes (`llm-pi-ai`,
`llm-deepseek`, `llm-deepseek-account`, `deepseek-llm-api-extensions`,
`deepseek-account`, and the two plugins that wait on them). Those providers
expect `DEEPSEEK_API_KEY`; this harness reads `AI_API_KEY` only, and the cleanest
way to guarantee that is for them not to be mounted at all.

Everything in that file is **overridable from a profile patch or a per-session
overlay** — which is how `App/scripts/run-acp.mjs` sets the workspace's route and
the absolute path of its models document without touching source.

### dsh packages this repo links directly

From `App/dsh-plugins/*/package.json` (`link:` into the checkout — these are why
the clone path is fixed):

```
vendor/cordis                            vendor/schemastery
packages/credentials/credentials          packages/util/launch-environment
packages/util/brand                       packages/llm/llm
packages/llm/llm-pi-ai                    packages/llm/token-meter
packages/core/agent                       packages/core/session
packages/core/system-prompt               packages/core/tools
packages/session/session-projection       packages/skill/skill
packages/subagent/subagent                packages/interaction/user-approval
```

## 5. Facts that cost real debugging time

1. **No `--acp` flag.** The composition decides; see §2.
2. **Stdout is protocol only.** A stray `console.log` in a plugin corrupts the
   ACP stream. Diagnostics go to stderr (`console.error`, or the logger).
3. **The first session's route is data, not a default.** The acp row ships a
   provider this composition disables, so a launch must patch a route in or the
   session cannot open. `run-acp.mjs` does it in this order: the workspace's
   `.harness/models.json` `active` entry → the repo's document → the deployment
   fallback (`AI_HARNESS_FALLBACK_MODEL` / `_BASE_URL`) → the provider's
   placeholder.
4. **A provider plugin resolves a relative `modelsFile` against the engine's
   cwd** (always the repo), so after a folder switch a relative path reads the
   wrong document. The generated overlay carries an **absolute** path; do not
   "simplify" it back.
5. **A route outside the installed catalog must declare its api**
   (`"api": "openai-completions"`), or `session/new` fails with *"needs an api;
   the installed catalog does not describe it"*.
6. **`session/new` takes a folder.** That single field is how the app works in
   any workspace: the engine's cwd stays the repo (for the `.env` layer and the
   profile), while the *work* happens wherever the client says.
7. **The engine is slow to start cold** (~5 s to `initialize`, ~15–20 s to a
   first session here, because tsx compiles the plugin graph). Keep the process
   across tasks; only restart when the route registry must change.
8. **dsh's own LLM providers expect `DEEPSEEK_API_KEY`.** That credential name
   must never appear in this project; those providers are mounted off, and our
   provider reads `AI_API_KEY` exclusively.

## 6. Where to look upstream for what

| You want to change… | Read first in the checkout |
|---|---|
| how the agent loop steps, compacts, and stops | `packages/core/agent/` |
| the session object, events, and projections | `packages/core/session/`, `packages/session/session-projection/` |
| the system prompt and its sections | `packages/core/system-prompt/` |
| tool definition, schemas, permissions | `packages/core/tools/`, `packages/interaction/user-approval/` |
| providers and the model catalog | `packages/llm/llm/`, `packages/llm/llm-pi-ai/` |
| token accounting | `packages/llm/token-meter/` |
| subagents: spawn vs fork, roles, tool masks | `packages/subagent/subagent/` |
| skills (markdown procedures the agent loads) | `packages/skill/skill/` |
| credentials resolution | `packages/credentials/credentials/` |
| the ACP bridge itself | `apps/` (the ACP app the bundle names) |
| profiles, bundles, patches, overlays | `docs/`, plus the scaffold `init-profile.mjs` generates |

The plugin directories carry their own tests — reading a plugin's test file is
usually faster than reading its source.

## 7. Upgrading dsh (a deliberate task, not a side quest)

The composition depends on internal package APIs, not only on ACP. An upgrade is
therefore: bump the tag in `takeover.md` §1 and the clone command, re-run
`make setup` (the `link:` deps pick up new paths), then `make check`,
`make check-native`, `make check-engine`, and the two `--ignored` end-to-end
tests. Expect breakage in the plugin services named in §4 — `dsh-*` packages are
in developer preview and say so themselves. Until that passes, the pinned tag is
the supported one.
