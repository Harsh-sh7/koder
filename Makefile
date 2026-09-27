# AI Harness — standardised evaluation interface. Lives at the repository root,
# as required by the submission guideline.
#
# Layout:
#   WARP/      vendored warpdotdev/warp checkout — read-only, never edited
#   Deepseek/  vendored deepseek-ai/deepseek-harness checkout — read-only, never edited
#              (neither checkout is tracked by this repository; `make setup`
#              automatically restores Deepseek/ when missing)
#   App/       ALL of our own code: the dsh plugins (agent brain), the native app,
#              the profile bootstrap, and the probes. Nothing we write goes inside
#              WARP/ or Deepseek/ directly.
#
# What runs:
#   The native application is the primary entry point.
#   `make run` builds the release application and launches it.
#
# The harness engine is Deepseek/deepseek-harness running from source
# (`node --import tsx apps/cli/src/bin.ts`) against the `hackathon-harness` dsh
# profile that App/scripts/init-profile.mjs materializes under $DSH_HOME.
#
# That profile composes three bundles — dsh-base, dsh-acp-app, and our own
# hackathon-harness-preset — and the preset is where this application's
# composition lives: route ownership, the frozen session spec, the operating
# contract, the wave/committee tools, and the budget cap.
#
# No file in Deepseek/ or WARP/ is modified, and no source change is needed
# to configure a model:
#   - AI_API_KEY is the only credential
#   - routes are declared in cordis.yml
#   - the route a session starts on is data in .harness/models.json
#
# Credential:
#   Reads ONLY AI_API_KEY.
#   Deepseek's own LLM providers default to DEEPSEEK_API_KEY
#   (see Deepseek/AGENTS.md); those providers are mounted off by our bundle,
#   and the provider plugin we mount reads AI_API_KEY exclusively.
#
# Never hardcode a credential in this file.

SHELL := /bin/sh

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

DSH_REPO := $(ROOT)/Deepseek/deepseek-harness
DSH_VERSION := dsh-v0.1.7-rc.2
DSH_URL := https://github.com/deepseek-ai/deepseek-harness.git

APP := $(ROOT)/App

# A node the engine can run on.
#
# dsh declares:
#   node ^22.19 or >=24
#
# When the interpreter on PATH is older (or missing), App/scripts/provision-node.sh
# installs that exact release into .toolchain/ — pinned version, checksum from the
# release's own SHASUMS256.txt.
#
# Every node-using recipe below runs through App/scripts/with-node.sh, which puts
# the provisioned node first on PATH so pnpm and the engine's child processes
# inherit it too.
#
# The same wrapper provides pnpm when the machine has none through Corepack,
# which ships with that node.
#
# Nothing is downloaded when the machine already has a compatible node + pnpm.

NODE := sh "$(APP)/scripts/with-node.sh"

# rustup installs into ~/.cargo/bin and only puts it on PATH through a line in
# the shell profile. A fresh install (or non-login shell) can therefore have
# cargo on disk but not on PATH.
#
# Every cargo recipe puts ~/.cargo/bin first on PATH.

CARGO := PATH="$(HOME)/.cargo/bin:$$PATH" cargo


.PHONY: setup run app app-release test typecheck check check-native check-engine check-reliability clean


# ---------------------------------------------------------------------------
# SETUP
# ---------------------------------------------------------------------------
#
# From a clean clone:
#   1. Restore the Deepseek harness if it is missing.
#   2. Ensure a compatible Node.js exists.
#   3. Install the Deepseek substrate dependencies.
#   4. Build the Deepseek native addon and host bundles.
#   5. Install our App packages.
#   6. Materialize the hackathon profile.
#
# Idempotent:
#   Re-running setup does not reclone an existing Deepseek checkout.
#   Profile link dependencies are rewritten to the current absolute paths.
#
# No manual Deepseek clone command is required.

setup:
	@echo "==> checking Deepseek/deepseek-harness..."
	@if [ ! -f "$(DSH_REPO)/package.json" ]; then \
		echo "==> Deepseek harness not found."; \
		echo "==> cloning $(DSH_VERSION)..."; \
		mkdir -p "$(ROOT)/Deepseek"; \
		rm -rf "$(DSH_REPO)"; \
		git clone --depth 1 --branch "$(DSH_VERSION)" \
			"$(DSH_URL)" "$(DSH_REPO)" || { \
				echo "setup: failed to clone Deepseek harness."; \
				exit 1; \
			}; \
	else \
		echo "==> Deepseek harness already present."; \
	fi

	@$(NODE) node -e 'console.log("==> node: " + process.version + " (" + process.execPath + ")")'

	@echo "==> substrate: Deepseek/deepseek-harness (vendored, installed as-is)"
	cd "$(DSH_REPO)" && $(NODE) pnpm install

	@echo "==> substrate build: native addon, host declarations, host bundles (~2 min the first time)"

	@echo "==> building native system addon..."
	cd "$(DSH_REPO)" && $(NODE) pnpm run build:native-system

	@echo "==> building host TypeScript declarations..."
	cd "$(DSH_REPO)" && \
		$(NODE) node --max-old-space-size=6144 \
		./node_modules/typescript/bin/tsc -b tsconfig.host.json

	@echo "==> building host bundle..."
	cd "$(DSH_REPO)" && \
		$(NODE) pnpm exec tsdown --env.DSH_BUILD_FACE host

	@echo "==> our packages: App/ (the plugin workspace)"
	cd "$(APP)" && $(NODE) pnpm install

	@echo "==> profile: hackathon-harness"
	$(NODE) node "$(APP)/scripts/init-profile.mjs"

	@echo ""
	@echo "setup: done."
	@echo "Run 'make run' to launch the AI Harness."


# ---------------------------------------------------------------------------
# RUN — PRIMARY USER ENTRY POINT
# ---------------------------------------------------------------------------
#
# `make run` is the one command users/reviewers need.
#
# It:
#   1. Builds the native application in release mode.
#   2. Launches the native application.
#
# The native application starts the engine itself when the agent panel needs it.
# Therefore we intentionally DO NOT start the ACP engine separately here.
#
# Cargo performs incremental builds, so subsequent runs are much faster.

run:
	@echo "==> building AI Harness..."
	cd "$(APP)/native" && $(CARGO) build --release -p harness-app

	@echo "==> launching AI Harness..."
	exec "$(APP)/native/target/release/ai-harness" --workspace "$(ROOT)"


# ---------------------------------------------------------------------------
# DEVELOPMENT APPLICATION
# ---------------------------------------------------------------------------
#
# Debug build of the native application.
#
# Useful during development because compilation is faster than a release build.

app:
	@echo "app: building debug application..."
	cd "$(APP)/native" && $(CARGO) build -p harness-app

	@echo "app: starting the window (Ctrl+C stops it)"
	exec "$(APP)/native/target/debug/ai-harness" --workspace "$(ROOT)"


# ---------------------------------------------------------------------------
# RELEASE APPLICATION
# ---------------------------------------------------------------------------
#
# Explicit release launcher.
#
# `make run` already performs this workflow, so this target is retained mainly
# for backwards compatibility and explicit developer use.

app-release:
	@echo "app-release: building release application..."
	cd "$(APP)/native" && $(CARGO) build --release -p harness-app

	@echo "app-release: starting the window (Ctrl+C stops it)"
	exec "$(APP)/native/target/release/ai-harness" --workspace "$(ROOT)"


# ---------------------------------------------------------------------------
# PLUGIN TESTS
# ---------------------------------------------------------------------------
#
# Covers:
#   - wave partitioner
#   - budget arithmetic
#   - provider models document
#   - spec lock
#   - preset composed over a real dsh session
#
# Runs offline against fakes.
#
# Tests import dsh packages the way the engine does — from source through the
# checkout's tsconfig path facade — so they do not require a built dsh workspace.

test:
	cd "$(APP)" && \
		TSX_TSCONFIG_PATH="$(DSH_REPO)/tsconfig.json" \
		$(NODE) pnpm run test


# ---------------------------------------------------------------------------
# TYPECHECK
# ---------------------------------------------------------------------------
#
# Plugins are shipped and loaded as TypeScript.
# Typechecking is therefore the build step for this half.

typecheck:
	cd "$(APP)" && $(NODE) pnpm run typecheck


# ---------------------------------------------------------------------------
# FULL CHECK
# ---------------------------------------------------------------------------
#
# Everything a reviewer can run without a model credential.
#
# Includes:
#   - plugin typecheck
#   - plugin tests
#   - native Rust workspace tests

check: typecheck test
	cd "$(APP)/native" && $(CARGO) test --workspace


# ---------------------------------------------------------------------------
# NATIVE SELF TEST
# ---------------------------------------------------------------------------
#
# Runs the native application's headless self-test.
#
# Verifies:
#   - workspace documents
#   - shell integration
#   - real PTY
#   - file tree
#   - search
#   - repository state

check-native:
	cd "$(APP)/native" && \
		$(CARGO) run -q -p harness-app -- \
		--workspace "$(ROOT)" \
		--check


# ---------------------------------------------------------------------------
# ENGINE SELF TEST
# ---------------------------------------------------------------------------
#
# Boots the ACP engine and opens a session.
#
# Requires:
#   make setup
#
# No real model credential is required when using the mock key.

check-engine:
	cd "$(APP)/native" && \
		$(CARGO) run -q -p harness-app -- \
		--workspace "$(ROOT)" \
		--check \
		--engine


# ---------------------------------------------------------------------------
# RELIABILITY CHECK
# ---------------------------------------------------------------------------
#
# Real engine + deterministic local provider.
#
# Covers:
#   - delegation
#   - restart
#   - route changes
#   - key changes

check-reliability:
	$(NODE) node "$(APP)/probe/reliability.mjs"


# ---------------------------------------------------------------------------
# CLEAN
# ---------------------------------------------------------------------------
#
# Removes generated launch artifacts and App-generated build/test artifacts.
#
# Does NOT remove:
#   - Deepseek/
#   - WARP/
#
# Those are vendored/reference checkouts and are intentionally preserved.

clean:
	rm -rf "$(ROOT)/.dsh/launch"
	cd "$(APP)" && $(NODE) pnpm run clean