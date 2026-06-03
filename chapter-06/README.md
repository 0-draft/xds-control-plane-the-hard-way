# Chapter 6: ORCA out-of-band load reports

*English | [日本語](./README.ja.md)*

Two backends report a synthetic utilization over ORCA. The lighter one ends up
serving the majority of traffic. In our run, a busy backend (utilization 0.9)
took 20 of 200 requests and the idle one (0.1) took 180, a clean 1:9 split.

## A straight answer about where ORCA runs

The original goal was Envoy's `client_side_weighted_round_robin` consuming
out-of-band ORCA directly. **Envoy doesn't do that.** The policy parses
`enable_oob_load_report`, but the wiring to actually receive `OrcaLoadReport`s
from hosts and weight endpoints by them was
[closed as not planned](https://github.com/envoyproxy/envoy/issues/34781). Set
it and you get plain round robin: a 50/50 split, no ORCA stream ever opened.

So the out-of-band ORCA client runs where it can: in the control plane. This is
the Hard Way reading of ORCA. The backends still expose a real
`OpenRcaService`; the control plane is the OOB agent that streams from it,
converts utilization into EDS weights, and pushes them. Envoy's ordinary
weighted round robin does the rest. The day Envoy consumes ORCA itself, the
backend half of this chapter is already done.

## What's new since Chapter 1

- The **backend serves two things on one h2c port**: HTTP data (with an
  `x-upstream` header) and the `xds.service.orca.v3.OpenRcaService` gRPC. Its
  `StreamCoreMetrics` streams an `OrcaLoadReport` whose `application_utilization`
  is fixed by `ORCA_UTILIZATION`. The proto is vendored and generated with
  tonic-build, because xds-api 0.2 has no ORCA types.
- The **control plane is an OOB ORCA client**. It streams from each backend,
  and on every reading recomputes an EDS `load_balancing_weight` (lighter
  backend, larger weight) and re-pushes EDS over a `tokio::sync::watch` channel.
- Two backends sit behind a normal EDS cluster; Envoy's default weighted round
  robin honours the weights.

## Run it

```bash
make up
make smoke       # converge, let ORCA settle, then prove the idle backend wins
make dist        # sample 200 requests and print the per-backend split
make logs        # watch ORCA reports turn into EDS weights
make down
```

## What it looks like

The control plane log shows ORCA turning into weights:

```text
INFO ORCA OOB client connected host=upstream-a
INFO ORCA OOB client connected host=upstream-b
INFO ORCA report host=upstream-b utilization=0.1
INFO ORCA report host=upstream-a utilization=0.9
INFO pushing EDS weights from ORCA version=v2 weights=["upstream-a=100", "upstream-b=900"]
INFO pushing config ty="EDS" version=v2
```

and the resulting traffic split:

```text
upstream-a (busy 0.9): 20    upstream-b (idle 0.1): 180
```

Two things to read out of that.

**Utilization becomes weight.** `weight = round((1 - utilization) * 1000)`, so
0.9 maps to 100 and 0.1 to 900. The 1:9 weight ratio shows up directly in the
1:9 request split, because Envoy's weighted round robin is just doing the
arithmetic on the EDS weights.

**The control plane closes the loop.** It both reads load (ORCA client) and acts
on it (EDS push). Nothing about the load is visible to Envoy except the weight,
which is exactly the boundary xDS draws: the data plane executes, the control
plane decides.

## How the code is laid out

```text
chapter-06/
├── controlplane/
│   ├── proto/                ORCA protos (client generated here)
│   ├── build.rs              tonic-build: OpenRcaService client
│   └── src/
│       ├── main.rs           ORCA client tasks + EDS watch-push server
│       └── snapshot.rs       EDS endpoints carry load_balancing_weight
├── upstream/
│   ├── proto/                same ORCA protos (server generated here)
│   ├── build.rs              tonic-build: OpenRcaService server
│   └── src/main.rs           h2c: HTTP data + ORCA StreamCoreMetrics
├── envoy/bootstrap.yaml      unchanged from Chapter 1
├── docker-compose.yml        upstream-a (0.9) and upstream-b (0.1)
└── Makefile
```

## What's intentionally missing

| Missing                                            | Where it lands |
| -------------------------------------------------- | -------------- |
| Envoy consuming ORCA in the data plane             | upstream Envoy |
| In-band ORCA (response trailers)                   | exercise       |

## Pinned versions

Same pins as Chapter 1, plus axum (the backend's h2c mux) and tonic-build (ORCA
codegen). The Dockerfile build stage adds `protobuf-compiler` for protoc.

| Dependency               | Pin             |
| ------------------------ | --------------- |
| `xds-api`                | `0.2`           |
| `tonic` / `tonic-build`  | `0.12`          |
| `prost` / `prost-types`  | `0.13`          |
| `tokio`                  | `1.41`          |
| `axum`                   | `0.7`           |
| Envoy image              | `v1.32-latest`  |
| Rust toolchain (build)   | `1.96-slim`     |

## References

- [ORCA: Open Request Cost Aggregation (`xds.data.orca.v3`)](https://github.com/cncf/xds/blob/main/xds/data/orca/v3/orca_load_report.proto)
- [Envoy issue 34781: receiving ORCA reports from hosts (closed, not planned)](https://github.com/envoyproxy/envoy/issues/34781)
- [Envoy client-side weighted round robin](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/load_balancing_policies/client_side_weighted_round_robin/v3/client_side_weighted_round_robin.proto)
