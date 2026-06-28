# chess-base — build/test/run entry points.
# Frontend recipes source nvm and honor frontend/.nvmrc (Node 22).

SHELL := /usr/bin/env bash
CARGO_BUILD_JOBS ?= 4
export CARGO_BUILD_JOBS

# Load nvm (if installed) and select the project Node version.
NVM = export NVM_DIR="$$HOME/.nvm"; [ -s "$$NVM_DIR/nvm.sh" ] && . "$$NVM_DIR/nvm.sh"; nvm use >/dev/null 2>&1 || true;

.PHONY: help
help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

## --- Frontend ---

.PHONY: deps
deps: ## Install frontend dependencies
	cd frontend && $(NVM) npm install

.PHONY: frontend
frontend: ## Build the Vue SPA into frontend/dist (embedded by the binary)
	cd frontend && $(NVM) npm run build

## --- Build / run ---

.PHONY: build
build: frontend ## Build the release binary (with embedded frontend)
	cargo build --release

.PHONY: run
run: frontend ## Run locally (SQLite, opens a browser)
	cargo run --

.PHONY: dev
dev: ## Run backend (:3030) + Vite dev server with hot reload
	@echo "Backend: cargo run -- --port 3030   |   Frontend: cd frontend && npm run dev"
	cargo run -- --port 3030 --no-open & \
	cd frontend && $(NVM) npm run dev

## --- Quality ---

.PHONY: test
test: test-unit test-int test-frontend ## Run all tests

.PHONY: test-unit
test-unit: ## Rust unit tests (lib)
	cargo test --lib

.PHONY: test-int
test-int: ## Rust integration tests (tests/)
	cargo test --test '*'

.PHONY: test-frontend
test-frontend: ## Frontend unit tests
	cd frontend && $(NVM) npm run test

.PHONY: coverage
coverage: ## Coverage for backend (llvm-cov) and frontend (vitest)
	cargo llvm-cov --summary-only
	cd frontend && $(NVM) npm run coverage

.PHONY: lint
lint: ## Clippy + rustfmt check + eslint
	cargo clippy --all-targets -- -D warnings
	cargo fmt --check
	cd frontend && $(NVM) npm run lint

.PHONY: fmt
fmt: ## Format Rust code
	cargo fmt

.PHONY: clean
clean: ## Remove build artifacts
	cargo clean
	rm -rf frontend/dist frontend/coverage
