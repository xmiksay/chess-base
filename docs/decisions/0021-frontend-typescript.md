# 0021 — Frontend in TypeScript

**Context.** The frontend was scaffolded in plain JavaScript, which contradicts
the workspace standard ("strong use of TypeScript/Vue 3"). As the SPA grew —
a typed-by-contract `api` client, ~10 Pinia stores, pure `lib/` modules and ~18
SFCs all passing snake_case backend payloads around — the absence of static types
meant refactors and API-shape changes (e.g. the keyset cursors, the study move
tree, the engine WebSocket events) had no compile-time guard.

**Decision.** Migrate the entire frontend to **TypeScript**, type-checked with
**`vue-tsc -b`** under the strict `@vue/tsconfig` DOM base (project-reference
layout: `tsconfig.app.json` for `src/`, `tsconfig.node.json` for the Vite config).
Lint runs **typescript-eslint** (flat config; `no-undef` delegated to the type
checker). Source modules are `.ts`; SFCs use `<script setup lang="ts">`.

One shared module, **`src/types.ts`**, is the single source of truth for the
domain/API types (`Database`, `Study`/`MoveTree`, `GameRow`, `MoveStat`,
`EngineMessage`/`Score`, `ReplayPosition`, `ImportJob`, …). The `api.ts` client is
generic over response types and annotates every endpoint; stores, `lib/` and SFCs
import from `types.ts` rather than redefining shapes. Relative module imports are
extensionless (Vite/`bundler` resolution); `.vue` imports keep their extension.
chessground's nominal `Key`/`Dests` types are bridged at the `Board.vue` boundary
only.

**Consequences.**
- `vue-tsc -b` gates `npm run build` **and** `npm run lint`, so both CI and
  `make lint` fail on a type error. The backend↔frontend contract is now checked
  against `types.ts` at compile time.
- New frontend work is TypeScript by default; do not add `.js` under `frontend/src`.
  Keep `types.ts` in sync when a backend payload changes (same change, per the
  docs-in-sync rule).
- `allowJs` stays on (harmless once no `.js` remain) to keep future incremental
  conversions painless. Test files (`*.test.ts`) are type-checked too; mock the
  typed `api` via `vi.mocked(...)`.
- The migration was type-only — no runtime behavior or test assertions changed.
