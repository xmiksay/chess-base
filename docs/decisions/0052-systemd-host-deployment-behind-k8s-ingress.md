# ADR-0052 — Deploy as a systemd service on the home desktop, behind the k8s ingress

Status: accepted — supersedes [ADR-0037](0037-k8s-deployment-ghcr-bundled-image.md)'s
deployment path (the GHCR image and `docker.yml` remain for Compose / third parties)

## Context

ADR-0037 ran chess-base as a pod on the vpsFree k8s cluster with the shared
cluster Postgres. That VPS is small: heavy Stockfish analysis competes with every
other service on it, and the only guard was the pod's 2-CPU limit. The home
desktop has far more CPU, already runs a host PostgreSQL, and already serves a
public app this way — `agent.mmik.cz` is a systemd service on 192.168.1.2 reached
by the cluster's nginx ingress through the Turris `wg0` tunnel via a
selector-less Service + Endpoints.

The local database is meant to stay **slim**: only the owner's own games and
studies. Reference data (masters games) is queried from Lichess remotely, not
bulk-imported.

## Decision

1. **The app runs as `chess-base.service` on the desktop** (`deploy/chess-base.service`),
   still in **server mode** — users, OAuth for the claude.ai MCP connector, sharing
   and the anonymous tier are unchanged. It runs as a dedicated `chessbase` system
   user under the same sandbox as `agent.service` (`ProtectSystem=strict`,
   `ProtectHome`, `PrivateTmp`, `NoNewPrivileges`), with `StateDirectory` as its
   only writable path (bundled Stockfish extracts into `$HOME/.cache` there).
2. **The host PostgreSQL holds the data**, reached over the unix socket with peer
   auth as role `chessbase` (`/etc/chess-base.env`, template
   `deploy/chess-base.env.example`) — no password to manage. Fresh database; the
   cluster DB is not migrated.
3. **Port 3040**, not 3030, so the service never collides with `make run`/`make dev`
   on the same machine.
4. **The cluster only routes**: `deploy/k8s.yml` is a headless selector-less
   Service + Endpoints → `192.168.1.2:3040` + the existing `chessbase.mmik.cz`
   Ingress. It holds no secret, so it is committed (the gitignored
   `deploy.yml`/`deploy.example.yml` pair is gone).
5. **Engine load is capped by systemd** (`CPUQuota=200%`, `MemoryMax=2G`), the
   same budget the pod had, now protecting the dev desktop.
6. **Rollout is `make deploy`**: build with bundled Stockfish, install to
   `/usr/local/bin/chess-base`, restart. `make install-service` is the one-time
   bootstrap (user, role, DB, env file, unit); `make deploy-k8s` applies the
   routing.

## Consequences

- The public site is down whenever the desktop or the `wg0` tunnel is (see the
  dual-WAN gotcha in the infra notes) — accepted for a personal tool.
- A release no longer needs a `v*` tag + image push; deploying is a local build.
- One-time cutover (not automated, destructive on the cluster):
  `kubectl -n services delete deploy/chess-base svc/chess-base secret/chess-base configmap/chess-base`
  (a ClusterIP Service can't be patched to headless), then `make deploy-k8s`;
  drop the old `chessbase` DB/role from the cluster Postgres once satisfied.
