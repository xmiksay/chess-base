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

# Real file target: the run targets rebuild the SPA only when a frontend source
# is newer than the last build, instead of paying a full vite build every run.
FE_SRC := $(shell find frontend/src -type f) $(wildcard frontend/*.json frontend/*.ts frontend/index.html)
frontend/dist/index.html: $(FE_SRC)
	cd frontend && $(NVM) npm run build

## --- Build / run ---

.PHONY: build
build: frontend ## Build the release binary (with embedded frontend)
	cargo build --release

.PHONY: release
release: frontend ## Build the locked, self-contained release binary for this host
	cargo build --release --locked

.PHONY: run
run: frontend/dist/index.html ## Run locally (SQLite, opens a browser)
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
	dir="engines-bundled/$$target"; \
	if [ -x "$$dir/$$bin" ]; then echo "$$dir/$$bin already bundled (rm -rf engines-bundled to refetch)"; exit 0; fi; \
	mkdir -p "$$dir"; \
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

.PHONY: run-bundled
run-bundled: frontend/dist/index.html bundle-stockfish ## Run locally with Stockfish embedded in the binary
	cargo run --features bundled-stockfish --

.PHONY: dev
dev: ## Run backend (:3030) + Vite dev server with hot reload
	@echo "Backend: cargo run -- --port 3030   |   Frontend: cd frontend && npm run dev"
	cargo run -- --port 3030 --no-open & \
	cd frontend && $(NVM) npm run dev

## --- Deploy (systemd on this host + k8s ingress, ADR 0052) ---

.PHONY: install-service
install-service: ## One-time: chessbase user, local Postgres role/DB, env file, systemd unit
	id chessbase >/dev/null 2>&1 || sudo useradd --system --home-dir /var/lib/chess-base --shell /usr/bin/nologin chessbase
	sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='chessbase'" | grep -q 1 || sudo -u postgres createuser chessbase
	sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='chessbase'" | grep -q 1 || sudo -u postgres createdb -O chessbase chessbase
	[ -f /etc/chess-base.env ] || sudo install -m 0640 -g chessbase deploy/chess-base.env.example /etc/chess-base.env
	sudo install -m 0644 deploy/chess-base.service /etc/systemd/system/chess-base.service
	sudo systemctl daemon-reload
	sudo systemctl enable chess-base

.PHONY: deploy
deploy: build-bundled ## Build with bundled Stockfish, install the binary, restart the service
	sudo install -m 0755 target/release/chess-base /usr/local/bin/chess-base
	sudo systemctl restart chess-base
	systemctl --no-pager --lines=5 status chess-base

.PHONY: deploy-k8s
deploy-k8s: ## Apply the k8s Service/Endpoints/Ingress pointing chessbase.mmik.cz at this host
	kubectl apply -f deploy/k8s.yml

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
