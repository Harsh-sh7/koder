# AI Harness — instructions for Claude Code

Read these two files before doing anything, in this order:

1. **`context.md`** — what this project is, the three priorities (token
   efficiency → time efficiency → structure), the locked decisions, and what
   exists today.
2. **`takeover.md`** — the rules that must not break, how to get it running, the
   verification loop, the conventions, and the work queue.

Also relevant:

- **`deepseek_reference.md`** — the engine (`dsh`): the repository to clone, how
  our plugins mount into it, and the traps that cost real debugging time.
- **`warp_reference.md`** — the UI design reference, and which of its crates map
  to which of our panes.
- `App/Architecture.md`, `App/PRD.md`, `README.md` — long-form architecture,
  product spec, and the user-facing tour.

The two upstream checkouts (`Deepseek/`, `WARP/`) are **not in the tree** — they
are gitignored and restored with the commands in `takeover.md` §1. `Deepseek/` is
required to build and run anything; `WARP/` is optional reading.

Working rules, in one line each: `make setup && make run` must always work;
`AI_API_KEY` is the only credential and never appears in a committed file;
nothing is written into `WARP/` or `Deepseek/`; model configuration is data in
`App/dsh-plugins/hackathon-harness-preset/cordis.yml`, never a code change; the
UI must not lie about what the harness is doing; and nothing is reported as done
without running it and saying what the run showed.
