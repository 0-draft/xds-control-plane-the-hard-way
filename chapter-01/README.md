# Chapter 1: Hello, xDS

*English | [日本語](./README.ja.md)*

The smallest possible thing that earns the name "xDS control plane".

## What you'll build

```text
              :10000 (HTTP)
   curl  ─────────────────►  Envoy  ◄────── :18000 (gRPC ADS) ──── controlplane (Rust)
                                ▲                                       │
                                │       LDS / RDS / CDS / EDS over ADS  │
                                │                                       │
                                ▼                                       ▼
                            upstream (Rust hyper)  on  :9000  ◄───────  (snapshot v1)
```

- **upstream** : a tiny `hyper` server that replies `hello from <hostname>` on `:9000`
- **controlplane** : a `tonic` ADS server that holds a single snapshot `v1` with one Listener / Route / Cluster / Endpoint, hand-built from `xds-api` proto types
- **envoy** : official Envoy image that loads `bootstrap.yaml`, finds the controlplane via the only static cluster, and pulls the rest dynamically

When the stack converges:

```bash
$ curl http://localhost:10000/
hello from <upstream-container-id>
path: /
method: GET
```

## Run it

```bash
make up          # builds and starts the 3 containers
make smoke       # waits for Envoy to converge, then curls
make logs        # see SUBSCRIBE / ACK printed by the controlplane
make down
```

The controlplane logs are where the protocol shows itself:

```text
INFO resolved upstream host=upstream ip=172.28.0.2
INFO snapshot loaded version=v1
INFO xDS server listening addr=0.0.0.0:18000
INFO ADS stream opened peer=172.28.0.4:41684
INFO client subscribed   node=envoy-hardway-01 kind="SUB " ty=CDS resources=[]
INFO client subscribed   node=                 kind="SUB " ty=EDS resources=["upstream_cluster"]
INFO client accepted config                    kind="ACK " ty=CDS version=v1 nonce=cds-v1
INFO client subscribed                         kind="SUB " ty=LDS resources=[]
INFO client accepted config                    kind="ACK " ty=EDS version=v1 nonce=eds-v1
INFO client subscribed                         kind="SUB " ty=RDS resources=["primary_route"]
INFO client accepted config                    kind="ACK " ty=LDS version=v1 nonce=lds-v1
INFO client accepted config                    kind="ACK " ty=RDS version=v1 nonce=rds-v1
```

Two things to notice in that trace.

**The walk of the dependency graph is visible.** Envoy first subscribes to
`CDS` and `LDS` with empty `resource_names`, which is the wildcard form
("send me everything you have"). The response carries cluster `upstream_cluster`
and listener `primary_listener`. Envoy then reads those resources, sees that
the cluster uses `EDS` and the listener uses `RDS`, and subscribes to the
two specific names `upstream_cluster` and `primary_route`. Four ACKs follow.

**The `node=` field is only filled on the first request.** That's the
`set_node_on_first_message_only: true` knob in `envoy/bootstrap.yaml`. Envoy
sends the full `Node` once at the start of the stream, then omits it on every
subsequent message in the same stream. The control plane has to remember it.

## How the code is laid out

```text
chapter-01/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          ADS server impl + ACK/NACK logging
│   │   └── snapshot.rs      hand-built v1 snapshot from xds-api types
│   └── Cargo.toml
├── upstream/
│   ├── src/main.rs          hyper HTTP server on :9000
│   └── Cargo.toml
├── envoy/
│   └── bootstrap.yaml       Envoy config (one static cluster: the controlplane)
├── docker-compose.yml
└── Makefile
```

## What's intentionally missing

So you have a reason to read Chapter 2+:

| Missing                                            | Where it lands                  |
| -------------------------------------------------- | ------------------------------- |
| Mutable snapshot. v1 is hard-coded forever         | Chapter 2                       |
| A real NACK demo (push a broken config, observe rollback) | Chapter 2                |
| SDS for mTLS material                              | Chapter 3                       |
| Delta xDS instead of SotW                          | Chapter 4                       |
| Multiple authorities (`xdstp://`)                  | Chapter 5                       |
| ORCA OOB load reports influencing LB choices       | Chapter 6                       |

## Pinned versions

These are deliberate pins. `tonic`, `prost`, and `xds-api` move in lockstep,
so bump those three together and rebuild rather than one at a time.

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

- [xds-api on docs.rs](https://docs.rs/xds-api/)
- [Envoy xDS REST and gRPC Protocol](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol)
- [Envoy bootstrap config reference](https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/bootstrap/v3/bootstrap.proto)
