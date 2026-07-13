# 0037 — k8s deployment: public GHCR image with bundled Stockfish

- **Status:** Accepted
- **Date:** 2026-07-13

## Context

ADR-0016 established the server-mode Dockerfile + Compose stack, explicitly
leaving the UCI engine out of the image (auto-download or a mounted engine at
runtime). Running chess-base on the personal k8s cluster needs (a) a published
image and (b) manifests. The cluster already hosts sibling apps (chess-puzzles,
teacher) with an established pattern: public `ghcr.io/xmiksay/<name>` images,
a single-file manifest per app in the `services` namespace, shared Postgres
(`postgres:5432`), nginx ingress + cert-manager (`letsencrypt-prod`).

## Decision

1. **The Docker image bundles Stockfish** via the existing `bundled-stockfish`
   feature (ADR-0005 amendment): the backend build stage runs
   `make bundle-stockfish` (sf 16.1, x86-64-avx2) and compiles with the feature
   on; `build.rs` checksum-verifies the binary. At startup the engine extracts
   to the pod's writable FS (`~/.cache/chess-base/`) — no volume, no runtime
   download (`--no-engine-download`). This supersedes ADR-0016's "no engine in
   the container" note for the k8s path; Compose builds the same image and
   inherits the bundled engine.
2. **The image is public on GHCR** (`ghcr.io/xmiksay/chess-base`), pushed by
   `.github/workflows/docker.yml` on `main` pushes and `v*` tags
   (branch/short-SHA/semver tags via `docker/metadata-action`). Public is both
   the cluster's pull pattern (its pull secret covers a different registry) and
   the licensing answer: bundling Stockfish makes the image **GPLv3**, so it
   must be distributable anyway.
3. **Deployment is one plain manifest** (`deploy.yml`, no Helm): Secret
   (DATABASE_URL, placeholder — real value managed out-of-band), ConfigMap
   (RUST_LOG), Deployment, Service (80→3030), Ingress
   (`chessbase.mmik.cz`, TLS via cert-manager). Probes hit `GET /api/health`;
   a generous startupProbe covers first-boot migrations + engine extraction.
4. **The pod CPU limit (2 cores) is the engine-load guard.** Users can raise
   engine threads to 64 in settings; the cgroup limit — not app config — is
   what protects the nodes from heavy Stockfish calculations.

## Consequences

- Live analysis works in k8s with zero engine setup; the image is ~75 MB
  larger and GPLv3.
- `kubectl apply -f deploy.yml` overwrites the Secret with the placeholder —
  edit the real DATABASE_URL in before applying, or manage the Secret
  separately.
- AVX2 is assumed on the nodes (verified at rollout); a non-AVX2 cluster would
  need the `sse41-popcnt` Stockfish slug instead.
- `make deploy` / `make deploy-restart` wrap apply + rollout; image updates on
  `:main` need the restart (imagePullPolicy Always, no digest pinning).
