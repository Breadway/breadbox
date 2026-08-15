# AGENTS.md — Repo hygiene

Follow [`CONTRIBUTING.md`](CONTRIBUTING.md). Single-trunk: `main` plus short-lived `feature/` / `fix/` branches. `dev`/`beta` are bakery tracks (tags / main), not git branches.

## Remotes
- `origin` — Forgejo (`git.breadway.dev`) — authoritative.
- `github` — mirror. Day-to-day push `origin` only.

## Product
GTK4 app launcher + `breadbox-sync` icon cache. Theme via `bread-theme` (pin by tag on `git.breadway.dev`). Toggle uses `bread-utils::singleton`, not a homegrown PID file.

## Distribution
Bakery (`bakery.toml`). Forgejo `.forgejo/workflows/` is canonical; do not re-add a GitHub Actions release workflow.
