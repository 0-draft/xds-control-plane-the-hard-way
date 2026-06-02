# Chapter 4: Delta xDS

*English | [日本語](./README.ja.md)*

The same stack, on the incremental protocol. State-of-the-World resent every
resource of a type whenever anything changed. Delta sends only the resources
whose version moved, and names removals explicitly. Here we implement
`delta_aggregated_resources` and prove a one-resource change pushes one resource.

## What's new since Chapter 2

- The Envoy bootstrap uses `api_type: DELTA_GRPC`. That is the only bootstrap
  change.
- Every **resource carries its own version**. The snapshot is now
  `type_url -> (name -> entry)`, and each entry knows its version, instead of
  one version stamped across the whole snapshot.
- The control plane implements the **Delta ADS** RPC: it tracks per-stream
  subscriptions (wildcard and by-name), records what each client already has via
  `initial_resource_versions`, and emits `DeltaDiscoveryResponse`s containing
  only the changed resources.

## Run it

```bash
make up          # build and start the 3 containers
make smoke       # converge over Delta, bump only the route, prove a one-resource push
make logs        # watch delta SUBSCRIBE / ACK / push
make down
```

Drive it by hand:

```bash
curl -i http://localhost:10000/   # note the x-config-version response header
make bump                         # POST /bump -> changes only the RouteConfiguration
curl -i http://localhost:10000/   # x-config-version flipped, nothing else moved
make status
```

## What incremental looks like

The route config carries an `x-config-version` response header so the bump is
visible from outside. `make bump` changes only that resource. In the control
plane log, the convergence sends one resource per type, and the bump sends
exactly one:

```text
SUB  ty="CDS" subscribe=[]                 wildcard=true
pushing delta ty="CDS" resources=1 removed=0
SUB  ty="EDS" subscribe=["upstream_cluster"] wildcard=false
pushing delta ty="EDS" resources=1 removed=0
SUB  ty="LDS" subscribe=[]                 wildcard=true
pushing delta ty="LDS" resources=1 removed=0
SUB  ty="RDS" subscribe=["primary_route"]  wildcard=false
pushing delta ty="RDS" resources=1 removed=0
ACK  ty="RDS" nonce=rds-4
--- POST /bump ---
pushing delta ty="RDS" resources=1 removed=0
ACK  ty="RDS" nonce=rds-5
```

Three things to read out of that.

**Wildcard vs by-name subscriptions are explicit.** Envoy subscribes to CDS and
LDS with an empty name list (wildcard: "send me everything of this type"), then
to EDS and RDS by the specific names it learned from the cluster and listener.

**`initial_resource_versions` is how a reconnect avoids re-downloading.** A
client tells the server which `(name, version)` pairs it already holds; the
server skips anything still current. On a fresh stream this map is empty, so the
first response is a full set, one resource at a time.

**A one-resource change is a one-resource push.** After the bump, only RDS moves.
CDS, LDS, and EDS stay silent because their versions did not change. That is the
entire point of Delta, and it is what the smoke test asserts.

## How the code is laid out

```text
chapter-04/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          delta_aggregated_resources + per-stream subscription state
│   │   └── snapshot.rs      per-resource versioned snapshot; bump touches only RDS
│   └── Cargo.toml
├── upstream/                unchanged from Chapter 1
├── envoy/bootstrap.yaml     api_type: DELTA_GRPC (the only change)
├── docker-compose.yml
└── Makefile
```

The heart of the chapter is `send_delta`: for a type, it computes the wanted
resource set (wildcard expands to every name; otherwise the subscribed names),
compares each resource's version against what this stream was last sent, and
emits only the difference, plus removals. When nothing moved it sends nothing,
which is exactly a client-side ACK.

## What's intentionally missing

| Missing                                            | Where it lands |
| -------------------------------------------------- | -------------- |
| Multiple authorities (`xdstp://`)                  | Chapter 5      |
| ORCA OOB load reports influencing LB choices       | Chapter 6      |

## Pinned versions

Same pins as Chapter 2.

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

- [Envoy: Incremental xDS (Delta)](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol#incremental-xds)
- [`DeltaDiscoveryRequest` / `DeltaDiscoveryResponse`](https://www.envoyproxy.io/docs/envoy/latest/api-v3/service/discovery/v3/discovery.proto)
