---
name: refresh-hardway
description: |
  Maintenance pass for xDS Control Plane the Hard Way. Bumps version pins
  (xds-api, tonic, prost, hyper, tokio, Envoy image, Rust toolchain),
  rebuilds each chapter, runs the smoke test, and surfaces a diff for
  review. Use this on a schedule (monthly) or before any release to
  prevent the Hard Way trap of sample code rotting against upstream.
metadata:
  type: skill
---

# Refresh xDS Control Plane the Hard Way

## When to invoke

Run this skill when:

- A reader reports the smoke test failing on a fresh clone.
- CI's scheduled drift cron (weekly) goes red.
- Before publishing a new chapter, to make sure earlier chapters still build.
- Once a month as routine hygiene.

Do NOT run it casually mid-chapter authoring; the skill mutates `Cargo.toml`
and `docker-compose.yml` pins, which conflicts with branch-in-progress work.

## What it does

1. **Inventory**. Walk the repo to enumerate every version pin:
   - `chapter-*/Cargo.toml` workspace dependencies
   - `chapter-*/docker-compose.yml` image tags
   - `chapter-*/Dockerfile` base image tags (`rust:X-slim`, `debian:codename-slim`)
   - `.github/workflows/ci.yml` toolchain version
2. **Discover upstream stable versions**.
3. **Update + verify per chapter** (each chapter is independent).
4. **Report**. Either commit a clean refresh or surface the breakage.

## Procedure

### Step 1: Snapshot the current state

Run from repo root:

```bash
grep -rnE '^(version|xds-api|tonic|prost|hyper|tokio|envoyproxy/envoy)' \
  chapter-*/Cargo.toml chapter-*/docker-compose.yml chapter-*/Dockerfile 2>/dev/null
```

Record the current pins. The output is the baseline you're moving off of.

### Step 2: Look up current upstream stable

Use these sources of truth in this order. If any one disagrees with another,
trust the crates.io / Docker Hub canonical and note the disagreement.

| What                  | Where to look                                                              |
| --------------------- | -------------------------------------------------------------------------- |
| `xds-api`             | `cargo search xds-api --limit 1` (returns `xds-api = "X.Y.Z"`)             |
| `tonic`               | `cargo search tonic --limit 1`. Mind that `xds-api` constrains the major.  |
| `prost` / `prost-types` | `cargo search prost --limit 1`. Major must match tonic's requirement.    |
| `tokio`               | `cargo search tokio --limit 1`. Stay on `1.x`.                             |
| `hyper`               | `cargo search hyper --limit 1`. We target `1.x`.                           |
| Envoy image           | `docker pull envoyproxy/envoy:contrib-latest && docker image inspect ... `, OR fetch `https://hub.docker.com/v2/repositories/envoyproxy/envoy/tags/?page_size=20` and pick the highest `vX.Y-latest` |
| Rust toolchain        | `rustup check`, OR `curl -s https://static.rust-lang.org/dist/channel-rust-stable.toml \| grep -m1 "^version"` |

Pin to the most recent `X.Y` (minor), not `X.Y.Z`, for crates that follow
SemVer. For Envoy, pin to `vX.Y-latest` (the moving minor tag), not
`vX.Y.Z` (exact patch).

### Step 3: Apply updates per chapter

For each `chapter-N/`:

1. `cd chapter-N`
2. Update `Cargo.toml` workspace dependencies to the new pins.
3. Update `docker-compose.yml` Envoy image tag.
4. Update `Dockerfile`(s) base image tag if Rust moved minors.
5. Run:

   ```bash
   cargo update
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo check --workspace --all-targets
   ```

6. If any step fails:
   - **proto field renamed/removed** (most common with `xds-api` bumps): grep the
     compiler error for the field name, then grep `~/xds` (the cncf/xds repo
     clone, if present) and the released `xds-api` source for the new name.
     Patch the call sites in `src/snapshot.rs`. Re-run.
   - **HCM / filter struct moved**: same playbook. The xds-api module path tracks
     `envoyproxy/data-plane-api` exactly.
   - **tonic API drift**: usually the generated server trait signature changes
     between `0.12 -> 0.13` etc. Patch `impl AggregatedDiscoveryService for ...`
     accordingly.
7. Once `cargo check` passes:

   ```bash
   docker compose down -v --remove-orphans
   docker compose up -d --build
   make smoke
   ```

   If smoke fails, dump `docker compose logs controlplane envoy` and diagnose.
   Common failures:
   - Envoy parses the bootstrap but cannot reach `controlplane:18000`: the
     controlplane container crashed on startup, check its logs.
   - LDS/CDS arrive but RDS/EDS never get subscribed: a routing dependency in
     the snapshot is wrong (cluster name typo, etc.).

### Step 4: Commit policy

- **All chapters pass**: one commit per chapter, message form
  `chore(chapter-N): refresh pins to <date>`. Body lists the old → new
  versions per crate / image.
- **Some chapters fail**: still commit the passing ones individually. For the
  failing chapter, do NOT commit broken code. Open a draft PR titled
  `[refresh] chapter-N regression on <upstream> bump`, with the compiler
  output or smoke log in the body. Stop and ask the user before forcing a fix.

Do NOT:

- Touch `Cargo.lock`s by hand. They regenerate from `cargo update`.
- Bump multiple chapters' Envoy pins to different versions. Keep them aligned.
- Skip `cargo clippy` warnings by adding `#[allow(...)]`. Either fix or surface.

### Step 5: Update the chapter README pin table

Each chapter README has a "Pinned versions" section. Update those tables too,
otherwise the docs drift from the actual Cargo manifests.

## Output contract

When you finish, report to the user with:

```text
Refresh pass: <YYYY-MM-DD>

Chapter 01:
  xds-api      0.2  -> 0.3
  tonic        0.12 -> 0.13
  Envoy        v1.32-latest -> v1.34-latest
  Status: clean (cargo check + smoke OK)
  Commit: <sha>

Chapter 02:
  Status: skipped (planned, not yet implemented)

Drift findings:
  - <anything weird worth flagging>
```

## Notes on the trap this skill exists to fix

K8s-the-Hard-Way and similar repos quietly rot because:

- crates.io / Envoy keep moving, but the repo has no CI heartbeat
- the proto path for one filter type gets renamed; ten chapters break at once
- the author moves on; readers find errors that block them before learning anything

The defenses this repo has:

1. **CI cron** (weekly) reruns the smoke test against the current pins. If
   the pins are immutable and the world stays the same, this stays green.
2. **This skill** is the manual escape valve when the cron goes red, or when
   the user proactively wants to consume newer Envoy/xDS features.
3. **Pins are minor-level**, not patch-level. Patch releases of `xds-api` /
   `tonic` flow in automatically; major-minor bumps are gated through this
   skill.
