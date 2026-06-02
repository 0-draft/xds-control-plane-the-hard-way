# xDS Control Plane the Hard Way

<img src="docs/banner.png" alt="xDS Control Plane the Hard Way" width="100%">

*English | [日本語](./README.ja.md)*

This is a set of chapters that build an Envoy xDS control plane from scratch,
in the spirit of Kubernetes the Hard Way. No Istio, no service mesh framework.
Just Envoy, the xDS protocol, and a control plane you write by hand in Rust.

It is optimized for learning, which means taking the long route so that LDS,
RDS, CDS, EDS, ACK/NACK, ADS, Delta, `xdstp://`, and ORCA stop being words you
nod along to.

> These chapters are a lab, not a framework. Don't run them in production.

## The stack

- Envoy as the data plane, in Docker.
- A control plane you write by hand with `tonic`, using the
  [`xds-api`](https://crates.io/crates/xds-api) crate only for the generated
  protobuf.
- A small upstream HTTP server in Rust (`hyper`).
- `docker compose` and `make` to wire it together.

The ADS server, the snapshot, and the ACK/NACK handling are written by hand on
purpose. `xds-api` does nothing but generate code from the protobuf.

## Chapters

Each chapter is a complete stack. Start it with `make up` and tear it down with
`make down`.

1. [Hello, xDS](chapter-01/). The smallest stack that earns the name: one
   listener, route, cluster, and endpoint served over ADS. Curl the upstream
   and watch the four ACKs go by.
2. [Snapshot swaps and rollback](chapter-02/). Push a broken listener, watch
   the version roll back on NACK.
3. [Static SDS to start mTLS](chapter-03/). Inject a cert over SDS and watch it
   hot-rotate.
4. Delta xDS. The same stack on the Delta protocol. (planned)
5. `xdstp://` and multiple authorities. A federated bootstrap and a second
   control plane. (planned)
6. ORCA out-of-band load reports. A backend reports synthetic load and the
   balancer picks the lighter pod. (planned)

## License

MIT. See [LICENSE](./LICENSE).
