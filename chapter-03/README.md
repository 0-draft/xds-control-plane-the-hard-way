# Chapter 3: Static SDS to start mTLS

*English | [日本語](./README.ja.md)*

The Listener now terminates TLS, and the server certificate is delivered as its
own SDS `Secret` resource over the same ADS stream. Because the cert is just
another pushable resource, rotating it is the same machinery as a snapshot swap:
the cert changes on a live listener with no Envoy restart.

## What's new since Chapter 2

- The Listener's filter chain has a **TLS transport socket**. Its
  `DownstreamTlsContext` points at an SDS secret by name (`server_cert`), it
  carries no cert bytes itself.
- The control plane serves a new resource type, **SDS `Secret`**, over ADS.
  Envoy subscribes to `server_cert` and we answer with the current certificate.
- `POST /rotate` **hot-rotates** the cert: mint a fresh self-signed leaf, bump
  the version, push a new snapshot. Envoy swaps it on the live listener.

## A wire-compatibility detour

xds-api 0.2 generates the SDS `Secret` types but *not* the listener-side
`DownstreamTlsContext` / `CommonTlsContext` wrappers. So `controlplane/src/xtls.rs`
defines the minimal subset by hand as prost messages, with field numbers that
match Envoy's real protos (`common_tls_context = 1`,
`tls_certificate_sds_secret_configs = 6`). The bytes are wire-compatible, so
Envoy decodes them under the real `DownstreamTlsContext` type URL. This is the
Hard Way earning its name: when the generated types stop short, you encode the
protobuf yourself.

## Run it

```bash
make up          # build and start the 3 containers
make smoke       # serve TLS via SDS, rotate the cert, prove the fingerprint changed
make logs        # watch SUBSCRIBE / ACK / SDS push
make down
```

Drive it by hand:

```bash
curl -k https://localhost:10000/      # TLS terminated with the SDS-delivered cert
make rotate                           # POST /rotate -> new cert on the live listener
make status                           # GET /status  -> advertised version
```

## What hot rotation looks like

`make smoke` records the server cert fingerprint, rotates, and reads it again:

```text
==> Body over TLS:
    hello from 2a7d729b2834
    path: /
    method: GET
==> Rotating the certificate on a live listener...
    before: sha256 Fingerprint=CD:07:9A:F6:D0:91:08:E5:...
    after:  sha256 Fingerprint=AA:30:E3:27:68:16:C7:F7:...
    OK: certificate hot-rotated without a restart
```

In the control plane log the secret rides the stream like any other resource.
Envoy subscribes to it by name right after LDS/RDS:

```text
INFO client subscribed ty="SDS" resources=["server_cert"]
INFO pushing config    ty="SDS" version=v1
INFO client accepted   kind="ACK " ty="SDS" version=v1 nonce=sds-v1
...
INFO pushing config    ty="SDS" version=v2
INFO client accepted   kind="ACK " ty="SDS" version=v2 nonce=sds-v2
```

Two things to read out of that.

**The cert is a resource, not a listener field.** LDS carries a reference
(`server_cert` over SDS), and SDS carries the bytes. Rotating touches only the
SDS resource; the Listener definition never changes. That separation is the
whole point of SDS, it lets certs rotate on a cadence the Listener never sees.

**Rotation is a version bump.** `POST /rotate` advertises a new snapshot, so the
secret moves v1 -> v2 and Envoy ACKs it. New TLS handshakes use the new leaf;
there is no restart and no dropped listener.

## How the code is laid out

```text
chapter-03/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          ADS push loop + admin /rotate
│   │   ├── snapshot.rs      now also builds the SDS Secret + TLS listener
│   │   ├── tls.rs           self-signed cert generation (rcgen)
│   │   └── xtls.rs          hand-rolled DownstreamTlsContext / CommonTlsContext
│   └── Cargo.toml           adds rcgen
├── upstream/                unchanged from Chapter 1
├── envoy/bootstrap.yaml     unchanged: the secret rides the existing ADS stream
├── docker-compose.yml
└── Makefile
```

## What's intentionally missing

This chapter does server-side TLS: Envoy presents a cert, the client does not.
Mutual TLS (requiring and validating a client cert via a second SDS validation
context) is the natural next increment and is left as an exercise.

| Missing                                            | Where it lands |
| -------------------------------------------------- | -------------- |
| Client-cert validation (full mTLS)                 | exercise       |
| Delta xDS instead of SotW                          | Chapter 4      |
| Multiple authorities (`xdstp://`)                  | Chapter 5      |
| ORCA OOB load reports influencing LB choices       | Chapter 6      |

## Pinned versions

Same pins as the earlier chapters, plus `rcgen` for cert generation. The
Dockerfile build stage adds `build-essential` because `rcgen`'s `ring` backend
compiles C.

| Dependency               | Pin             |
| ------------------------ | --------------- |
| `xds-api`                | `0.2`           |
| `tonic`                  | `0.12`          |
| `prost` / `prost-types`  | `0.13`          |
| `tokio`                  | `1.41`          |
| `hyper`                  | `1.5`           |
| `rcgen`                  | `0.13`          |
| Envoy image              | `v1.32-latest`  |
| Rust toolchain (build)   | `1.96-slim`     |

## References

- [Envoy SDS (Secret Discovery Service)](https://www.envoyproxy.io/docs/envoy/latest/configuration/security/secret)
- [Envoy `DownstreamTlsContext`](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/transport_sockets/tls/v3/tls.proto)
- [`rcgen` on docs.rs](https://docs.rs/rcgen/)
