# AI Harness — standardised evaluation interface. Lives at the repository root,
# as required by the submission guideline.
#
# Layout:
#   WARP/      vendored warpdotdev/warp checkout — read-only, never edited
#   Deepseek/  vendored deepseek-ai/deepseek-harness checkout — read-only, never edited
#              (neither checkout is tracked by this repository; a machine without
#              Deepseek/ cannot build or run — the clone command is in `setup`
#              below and in takeover.md section 1)
#   App/       ALL of our own code: the dsh plugins (agent brain), the native app,
#              the profile bootstrap, and the probes. Nothing we write goes inside
#              WARP/ or Deepseek/ directly.
#
# What runs: the harness engine is Deepseek/deepseek-harness running from source
# (`node --import tsx apps/cli/src/bin.ts`) against the `hackathon-harness` dsh
# profile that App/scripts/init-profile.mjs materializes under $DSH_HOME. That
# profile composes three bundles — dsh-base, dsh-acp-app, and our own
# hackathon-harness-preset — and the preset is where this application's
# composition lives: route ownership, the frozen session spec, the operating
# contract, the wave/committee tools, and the budget cap. No file in Deepseek/ or
# WARP/ is modified, and no source change is needed to configure a model:
# AI_API_KEY is the only credential, the routes are declared in cordis.yml, and
# the route a session starts on is data in .harness/models.json.
#
# Credential: reads ONLY AI_API_KEY. Deepseek's own LLM providers default to
# DEEPSEEK_API_KEY (see Deepseek/AGENTS.md); those providers are mounted off by
# our bundle, and the provider plugin we mount reads AI_API_KEY exclusively.
# Never hardcode a credential in this file.

SHELL := /bin/sh
ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
DSH_REPO := $(ROOT)/Deepseek/deepseek-harness
APP := $(ROOT)/App

# A node the engine can run on. dsh declares `node ^22.19 or >=24`; when the
# interpreter on PATH is older (or missing), App/scripts/provision-node.sh
# installs that exact release into .toolchain/ — pinned version, checksum from
# the release's own SHASUMS256.txt — and every node-using recipe below runs
# through App/scripts/with-node.sh, which puts it first on PATH so pnpm and the
# engine's own child processes inherit it too. The same wrapper provides a pnpm
# when the machine has none (corepack, which ships with that node). Nothing is
# downloaded when the machine already has both.
NODE := sh "$(APP)/scripts/with-node.sh"

# rustup installs into ~/.cargo/bin and only puts it on PATH through a line in
# the shell profile, so a fresh install (or a non-login shell) has cargo on disk
# but not on PATH. Every cargo recipe runs with that directory first on PATH, so
# cargo and the rustc it calls are found either way.
CARGO := PATH="$(HOME)/.cargo/bin:$$PATH" cargo

.PHONY: setup run app app-release test typecheck check check-native check-engine clean

# From a clean clone: an interpreter, the vendored substrate, our packages, and
# the profile. Idempotent — re-run it after moving the checkout and the
# profile's `link:` dependencies are rewritten to the new absolute paths.
#
# This is where node comes from: a machine that cannot run dsh's range
# (`^22.19 or >=24`) gets that release installed into .toolchain/ here, once,
# with its published checksum verified — and a machine without pnpm gets one
# through that node's own corepack. No container, no system-wide install.
setup:
	@[ -f "$(DSH_REPO)/package.json" ] || { echo "setup: Deepseek/deepseek-harness is missing its manifest. The vendored upstream checkouts are not tracked by this repository; restore the Deepseek checkout at the repo root before running setup:"; echo; echo "    git clone --depth 1 --branch dsh-v0.1.7-rc.2 https://github.com/deepseek-ai/deepseek-harness.git Deepseek/deepseek-harness"; echo; echo "  (WARP/ is a read-only reference only — nothing here builds or reads it. See takeover.md section 1.)"; exit 1; }
	@$(NODE) node -e 'console.log("==> node: " + process.version + " (" + process.execPath + ")")'
	@echo "==> substrate: Deepseek/deepseek-harness (vendored, installed as-is)"
	cd "$(DSH_REPO)" && $(NODE) pnpm install
	@# dsh runs from source, but not from an install alone: a session needs the
	@# native system addon and the host libraries' typert bundles, which only its
	@# own build produces (its README: `pnpm install` then `pnpm run build`). This
	@# is that build minus the web and desktop clients the ACP engine never loads.
	@# Incremental, so a re-run on a built checkout takes seconds. Needs a C
	@# compiler for the addon (Xcode command-line tools on macOS, cc on Linux).
	@echo "==> substrate build: native addon, host declarations, host bundles (~2 min the first time)"
	cd "$(DSH_REPO)" && $(NODE) pnpm run build:native-system
	cd "$(DSH_REPO)" && $(NODE) node --max-old-space-size=6144 ./node_modules/typescript/bin/tsc -b tsconfig.host.json
	cd "$(DSH_REPO)" && $(NODE) pnpm exec tsdown --env.DSH_BUILD_FACE host
	@echo "==> our packages: App/ (the plugin workspace)"
	cd "$(APP)" && $(NODE) pnpm install
	@echo "==> profile: hackathon-harness"
	$(NODE) node "$(APP)/scripts/init-profile.mjs"
	@echo "setup: done. 'make run' starts the harness engine on stdio (ACP)."

# Launch the engine and stay up. The profile's application is the ACP bridge, so
# this serves Agent Client Protocol on stdin/stdout until the client closes it;
# diagnostics go to stderr. An evaluation issue is supplied to this
# already-running process (an ACP session/prompt), not passed at launch.
run:
	@$(NODE) node "$(APP)/scripts/run-acp.mjs"

# The native application: the terminal-first interface over the same engine. It
# starts the engine itself when the agent panel needs one, so `make run` is not a
# prerequisite; opening the app on a workspace with no model route shows the empty
# state instead. Debug is quick to build; release is what a demo should use.
# The build and the launch are two commands on purpose: cargo's progress bar
# stays on screen when the build ends, so a single `cargo run` line leaves the
# terminal looking frozen while the window is already open. The line between
# them says the window is coming.
app:
	@echo "app: building if needed — the window opens when cargo finishes"
	cd "$(APP)/native" && $(CARGO) build -p harness-app
	@echo "app: starting the window (Ctrl+C stops it)"
	exec "$(APP)/native/target/debug/ai-harness" --workspace "$(ROOT)"

app-release:
	@echo "app-release: building if needed — a first release build takes a minute or two"
	cd "$(APP)/native" && $(CARGO) build --release -p harness-app
	@echo "app-release: starting the window (Ctrl+C stops it)"
	exec "$(APP)/native/target/release/ai-harness" --workspace "$(ROOT)"

# What the plugins' own tests cover: the wave partitioner, the budget arithmetic,
# the provider's models document, the spec lock, and the preset composed over a
# real dsh session. Runs offline against fakes. The tests import dsh packages the
# way the engine does — from source, through the checkout's tsconfig path facade
# — so they need no build of the dsh workspace (its `lib/` is never produced by
# `make setup`).
test:
	cd "$(APP)" && TSX_TSCONFIG_PATH="$(DSH_REPO)/tsconfig.json" $(NODE) pnpm run test

# The plugins ship as TypeScript and are loaded as TypeScript, so there is no
# compile step to run here — typechecking IS the build for this half. The native
# app has its own build (cargo, in App/native).
typecheck:
	cd "$(APP)" && $(NODE) pnpm run typecheck

# Everything a reviewer can run without a model credential, in one target. The
# native app's headless self-test is what proves the window would have something
# to draw: the workspace documents, the shell integration, a real PTY, the file
# tree, a search, and the repository.
check: typecheck test
	cd "$(APP)/native" && $(CARGO) test --workspace

# The engine half of that self-test: boot the ACP engine and open a session. It
# needs `make setup`, and it needs no credential — a session is opened, not
# prompted. With `AI_API_KEY` set to the mock key this is offline end to end.
check-native:
	cd "$(APP)/native" && $(CARGO) run -q -p harness-app -- --workspace "$(ROOT)" --check

# Boots the engine and opens a session over ACP. Needs `make setup`.
check-engine:
	cd "$(APP)/native" && $(CARGO) run -q -p harness-app -- --workspace "$(ROOT)" --check --engine

clean:
	rm -rf "$(ROOT)/.dsh/launch"
	cd "$(APP)" && $(NODE) pnpm run clean

# Real engine + deterministic local provider: delegation, restart and route/key changes.
.PHONY: check-reliability
check-reliability:
	$(NODE) node "$(APP)/probe/reliability.mjs"
