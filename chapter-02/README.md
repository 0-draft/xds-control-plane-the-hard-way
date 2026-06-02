# Chapter 2: Snapshot swaps and rollback

*English | [日本語](./README.ja.md)*

Chapter 1 served one frozen snapshot. Here the control plane changes its mind
at runtime, pushes the new version to Envoy without being asked, and rolls back
when Envoy says no.

## What's new since Chapter 1

- The snapshot is **versioned and mutable**. An admin HTTP API on `:19000`
  builds a fresh snapshot and advertises it.
- The ADS stream does **server-initiated push**. A `tokio::sync::watch` channel
  fans every snapshot change out to the connected Envoy, instead of only
  answering requests.
- **Rollback on NACK**. When Envoy rejects a pushed config, the control plane
  re-advertises the last-known-good snapshot, so `:10000` keeps serving.

## What you'll build

```text
                       :19000 (admin HTTP)
   operator ──POST /push/broken──►  controlplane (Rust)
                                         │  watch channel
              :10000 (HTTP)             ▼
   curl  ───────────────►  Envoy  ◄── :18000 (gRPC ADS) ── push v2 / NACK / rollback to v1
                              │
                              ▼
                          upstream (:9000)
```

## Run it

```bash
make up          # build and start the 3 containers
make smoke       # converge on v1, push a broken Listener, prove rollback, swap to a good version
make logs        # watch SUBSCRIBE / ACK / NACK / rollback
make down
```

Or drive the admin API by hand:

```bash
make push-broken   # POST /push/broken  -> advertises a Listener Envoy rejects
make push-good     # POST /push/good    -> a clean version swap
make status        # GET  /status       -> advertised vs last_good version
```

## What the rollback looks like

`make push-broken` advertises a Listener whose `HttpConnectionManager` has an
empty `stat_prefix`. Everything else in the snapshot is valid, so the NACK is
unambiguously about that one Listener. The control plane log:

```text
INFO pushing config ty="EDS" version=v2
INFO pushing config ty="RDS" version=v2
INFO pushing config ty="LDS" version=v2
INFO pushing config ty="CDS" version=v2
INFO client accepted config kind="ACK " ty="EDS" version=v2 nonce=eds-v2
INFO client accepted config kind="ACK " ty="RDS" version=v2 nonce=rds-v2
WARN client rejected config kind="NACK" ty="LDS" version=v1 nonce=lds-v2 msg=Error adding/updating listener(s) primary_listener: Proto constraint validation failed (HttpConnectionManagerValidationError.StatPrefix: value length must be at least 1 characters)
INFO rolling back after NACK rollback_to=v1
INFO client accepted config kind="ACK " ty="CDS" version=v2 nonce=cds-v2
INFO pushing config ty="LDS" version=v1
INFO client accepted config kind="ACK " ty="LDS" version=v1 nonce=lds-v1
```

Three things to read out of that trace.

**A version spans all four resource types.** Pushing v2 sends new CDS, EDS, RDS,
and LDS responses. The valid three get ACKed; only the Listener fails.

**The NACK echoes Envoy's own validation error.** `version=v1` on the NACK is
Envoy telling you the last version it actually accepted for LDS, not the version
it just rejected. The `msg` is the proto-validation failure verbatim, which is
how you'd debug a real broken push.

**Rollback is the control plane's job, not Envoy's.** Envoy already keeps the
last good Listener on a NACK, so traffic never drops. But the *advertised*
version is still the broken v2 until the control plane re-advertises v1. That
re-advertisement is the rollback, and it is what `last_good` exists for.

## How the code is laid out

```text
chapter-02/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          ADS push loop (watch), admin API, rollback
│   │   └── snapshot.rs      good() and broken() snapshot builders
│   └── Cargo.toml
├── upstream/                unchanged from Chapter 1
├── envoy/bootstrap.yaml     unchanged from Chapter 1
├── docker-compose.yml       controlplane now also exposes :19000
└── Makefile
```

The control plane holds two things that did not exist in Chapter 1: a
`watch::Sender<Arc<Snapshot>>` for the currently advertised snapshot, and a
`last_good` slot. `push_broken` advertises without touching `last_good`;
`push_good` updates both. On a NACK the stream re-advertises `last_good`, and
the watch arm of its `select!` loop pushes it back out.

## What's intentionally missing

| Missing                                            | Where it lands |
| -------------------------------------------------- | -------------- |
| SDS for mTLS material                              | Chapter 3      |
| Delta xDS instead of SotW                          | Chapter 4      |
| Multiple authorities (`xdstp://`)                  | Chapter 5      |
| ORCA OOB load reports influencing LB choices       | Chapter 6      |

## Pinned versions

Same pins as Chapter 1. `tonic`, `prost`, and `xds-api` move in lockstep, so
bump those three together and rebuild rather than one at a time.

| Dependency               | Pin             |
| ------------------------ | --------------- |
| `xds-api`                | `0.2`           |
| `tonic`                  | `0.12`          |
| `prost` / `prost-types`  | `0.13`          |
| `tokio`                  | `1.41`          |
| `hyper`                  | `1.5`           |
| Envoy image              | `v1.32-latest`  |
| Rust toolchain (build)   | `1.96-slim`     |

## References

- [Envoy xDS protocol: ACK/NACK and resource warming](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol#ack-nack-and-versioning)
- [tokio `watch` channel](https://docs.rs/tokio/latest/tokio/sync/watch/)
