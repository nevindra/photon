# Photon — local development.
#
#   make dev     run backend + Vue dev server together (process-compose TUI)
#   make help    list all targets
#
# `make dev` manages two processes (see process-compose.yaml): the Rust backend and the
# Vite dev server. The first run compiles the whole Rust workspace (DataFusion is large) —
# give it a minute. This tooling is for local development only.

SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

DEV_CONFIG := photon.dev.toml
DEV_DATA   := .photon-dev
DEV_USER   := admin
DEV_PASS   := admin

# process-compose runs its own control API. By default that's TCP :8080 — the SAME port
# Photon's API binds, so pc would claim it first and the `server` process fails with
# "address already in use". Run pc over a unix socket instead (never touches :8080); the
# pinned path lets `up`, `attach`, and `down` all find the same running project.
PC_SOCK := $(DEV_DATA)/pc.sock
export PC_SOCKET_PATH := $(PC_SOCK)

.PHONY: help
help: ## List available targets
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

## ---- Development ----------------------------------------------------------

.PHONY: dev
dev: check-tools $(DEV_CONFIG) frontend/node_modules ## Run backend + frontend (process-compose TUI)
	@mkdir -p $(DEV_DATA)/logs
	@rm -f $(PC_SOCK)
	process-compose up --use-uds

.PHONY: dev-detached
dev-detached: check-tools $(DEV_CONFIG) frontend/node_modules ## Run the dev stack in the background
	@mkdir -p $(DEV_DATA)/logs
	@rm -f $(PC_SOCK)
	process-compose up --use-uds --detached
	@echo "Stack running in background — attach: make attach   stop: make down"

.PHONY: attach
attach: ## Attach the TUI to a detached dev stack
	process-compose attach --use-uds

.PHONY: down
down: ## Stop a detached dev stack
	process-compose down --use-uds

.PHONY: logs
logs: ## Tail the per-process dev logs
	tail -n +1 -f $(DEV_DATA)/logs/server.log $(DEV_DATA)/logs/web.log

## ---- Build & test ---------------------------------------------------------

.PHONY: build-frontend
build-frontend: frontend/node_modules ## Build the Vue bundle into frontend/dist
	cd frontend && bun run build

.PHONY: build
build: build-frontend ## Release build of photon-server (frontend embedded)
	cargo build --release

.PHONY: test
test: ## Run the Rust test suite
	cargo test

.PHONY: fmt
fmt: ## Format Rust sources
	cargo fmt

.PHONY: clippy
clippy: ## Lint with clippy
	cargo clippy --all-targets

# Load-test a running server (e.g. `make dev`) with OTLP logs. Override the args, e.g.:
#   make loadtest LOADTEST_ARGS="logs --saturate --concurrency 64 --duration 60"
LOADTEST_ARGS ?= logs --rate 20000
.PHONY: loadtest
loadtest: ## Drive OTLP logs at a running server (override LOADTEST_ARGS=...)
	cargo run --release -p photon-loadgen -- $(LOADTEST_ARGS)

# Load-test a running server with OTLP traces. Override the args, e.g.:
#   make tracetest TRACETEST_ARGS="traces --saturate --concurrency 64 --duration 60"
TRACETEST_ARGS ?= traces --rate 2000
.PHONY: tracetest
tracetest: ## Drive OTLP traces at a running server (override TRACETEST_ARGS=...)
	cargo run --release -p photon-loadgen -- $(TRACETEST_ARGS)

## ---- Benchmark ------------------------------------------------------------

.PHONY: bench-micro
bench-micro: ## Run the write-path criterion micro-benches (ingest + wal)
	cargo bench -p photon-ingest --bench logs_ingest
	cargo bench -p photon-wal --bench append

.PHONY: bench-ingest
bench-ingest: ## End-to-end logs ingest throughput + peak RSS (PHOTON_BENCH_DIR=tmpfs by default)
	./scripts/bench-ingest.sh

.PHONY: bench-sweep
bench-sweep: ## Concurrency sweep — find the true ingest ceiling (CPU vs concurrency limited)
	./scripts/bench-sweep.sh

.PHONY: mem-profile
mem-profile: ## Memory profile — RSS over time + post-load idle (see scripts/mem-profile.sh env knobs)
	./scripts/mem-profile.sh

## ---- Docker deployment ----------------------------------------------------

.PHONY: docker-build
docker-build: ## Build the production image (photon:latest)
	docker build -t photon:latest .

.PHONY: docker-up
docker-up: ## Start the core stack (photon + volume) in the background
	docker compose up -d --build

.PHONY: docker-down
docker-down: ## Stop the stack (keeps the data volume)
	docker compose down

.PHONY: docker-up-durable
docker-up-durable: ## Start the stack WITH the Garage durable-S3 tier
	docker compose --profile durable up -d --build

## ---- Setup / housekeeping -------------------------------------------------

.PHONY: install-tools
install-tools: ## Install process-compose (dev process runner)
	./scripts/install-process-compose.sh

.PHONY: clean-dev
clean-dev: ## Delete dev config + local dev data (.photon-dev, photon.dev.toml)
	rm -rf $(DEV_DATA) $(DEV_CONFIG)

# ---- internal -------------------------------------------------------------

frontend/node_modules: frontend/package.json frontend/bun.lock
	cd frontend && bun install
	@touch $@

# A ready-to-run dev config: local data dir + throwaway secrets. There are no config users —
# the first `make dev` drops you at the one-time onboarding screen to create your account.
$(DEV_CONFIG):
	@echo ">> generating $(DEV_CONFIG) — first run: open the UI and create your account"
	@mkdir -p $(DEV_DATA)/hot
	@printf '%s\n' \
		'# Auto-generated dev config (make dev). Gitignored, throwaway — do NOT use in production.' \
		'' \
		'[ingest]' \
		'token = "dev-ingest-token"' \
		'http_addr = "127.0.0.1:4318"' \
		'grpc_addr = "127.0.0.1:4317"' \
		'' \
		'[storage]' \
		'hot_dir = "$(DEV_DATA)/hot"' \
		'db_path = "$(DEV_DATA)/photon.db"' \
		'' \
		'[retention]' \
		'days = 7' \
		'' \
		'[schema]' \
		'promoted_attributes = ["service.name", "host.name"]' \
		'' \
		'[wal]' \
		'segment_max_bytes = 134217728' \
		'segment_max_age_secs = 60' \
		'group_commit_max_delay_ms = 5' \
		'' \
		'[auth]' \
		'session_secret = "photon-dev-session-secret-not-for-production-0123456789"' \
		> $(DEV_CONFIG)
	@echo ">> wrote $(DEV_CONFIG)"

.PHONY: check-tools
check-tools:
	@command -v process-compose >/dev/null 2>&1 || { \
		echo "process-compose not found — install it with:  make install-tools"; exit 1; }
