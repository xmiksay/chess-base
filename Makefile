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

.PHONY: release
release: frontend ## Build the locked, self-contained release binary for this host
	cargo build --release --locked

.PHONY: run
run: frontend ## Run locally (SQLite, opens a browser)
	cargo run --

.PHONY: bundle-stockfish
bundle-stockfish: ## Fetch this host's Stockfish into engines-bundled/<target>/ (for --features bundled-stockfish)
	@set -e; \
	target=$$(rustc -vV | sed -n 's/host: //p'); \
	case "$$target" in \
	  x86_64-*-linux-*)   slug=stockfish-ubuntu-x86-64-avx2;      bin=stockfish;     arch=tar; inner=$$slug ;; \
	  aarch64-*-linux-*)  slug=stockfish-android-armv8;           bin=stockfish;     arch=tar; inner=$$slug ;; \
	  x86_64-apple-*)     slug=stockfish-macos-x86-64-avx2;       bin=stockfish;     arch=tar; inner=$$slug ;; \
	  aarch64-apple-*)    slug=stockfish-macos-m1-apple-silicon;  bin=stockfish;     arch=tar; inner=$$slug ;; \
	  x86_64-*-windows-*) slug=stockfish-windows-x86-64-avx2;     bin=stockfish.exe; arch=zip; inner=$$slug.exe ;; \
	  *) echo "no Stockfish asset catalogued for target $$target" >&2; exit 1 ;; \
	esac; \
	dir="engines-bundled/$$target"; mkdir -p "$$dir"; \
	url="https://github.com/official-stockfish/Stockfish/releases/download/sf_16.1/$$slug.$$arch"; \
	tmp=$$(mktemp -d); trap 'rm -rf "$$tmp"' EXIT; \
	echo "Fetching $$url"; \
	curl -fSL "$$url" -o "$$tmp/archive"; \
	case "$$arch" in \
	  tar) tar -xf "$$tmp/archive" -C "$$tmp" ;; \
	  zip) unzip -oq "$$tmp/archive" -d "$$tmp" ;; \
	esac; \
	cp "$$tmp/stockfish/$$inner" "$$dir/$$bin"; \
	chmod +x "$$dir/$$bin"; \
	( cd "$$dir" && { sha256sum "$$bin" || shasum -a 256 "$$bin"; } | awk '{print $$1}' > "$$bin.sha256" ); \
	echo "Bundled $$dir/$$bin (LICENSING: Stockfish is GPLv3 — a bundled build is GPLv3)"

.PHONY: build-bundled
build-bundled: frontend bundle-stockfish ## Build the release binary with Stockfish embedded (GPLv3 artifact)
	cargo build --release --features bundled-stockfish

.PHONY: dev
dev: ## Run backend (:3030) + Vite dev server with hot reload
	@echo "Backend: cargo run -- --port 3030   |   Frontend: cd frontend && npm run dev"
	cargo run -- --port 3030 --no-open & \
	cd frontend && $(NVM) npm run dev

## --- Deploy (k8s, ADR 0037) ---

.PHONY: deploy
deploy: ## Apply the k8s manifest (Secret/ConfigMap/Deployment/Service/Ingress)
	kubectl apply -f deploy.yml

.PHONY: deploy-restart
deploy-restart: ## Roll the pods onto the freshly pushed :main image
	kubectl -n services rollout restart deploy/chess-base
	kubectl -n services rollout status deploy/chess-base

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
