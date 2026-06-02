# Chapter 5: xdstp:// and multiple authorities

*English | [日本語](./README.ja.md)*

Every resource is now named with an `xdstp://` URL instead of a bare string, and
the names span two authorities: `hardway` owns the Listener and Route, `edge`
owns the Cluster and Endpoint. Envoy bootstraps the Listener and Cluster
collections by glob locator and follows the graph into RDS/EDS singletons,
entirely by xdstp name.

## A straight answer about federation first

The original goal for this chapter was "federated bootstrap, second control
plane": authority `hardway` served by one control plane, `edge` by another.
**Envoy cannot do that yet.** From the Envoy xDS design notes:

> We do not yet support federated configuration sources, it is assumed that a
> single ADS stream or `ConfigSource` specified parallel to the `xdstp://`
> resource locator is used. Envoy will support this in the future once a
> bootstrap based mapping from authority to `ConfigSource` is supported.

So an authority is, today, a naming construct, not a routing construct. Both
authorities resolve to the same ADS stream. This chapter builds what is real:
xdstp:// naming and glob collections over one Delta stream, with a single
control plane serving both authorities. The day Envoy ships authority-to-source
mapping, splitting `edge` onto a second control plane is a bootstrap change, not
a code change.

## What's new since Chapter 4

- Resources are named `xdstp://{authority}/{proto type}/{id}`, across the
  `hardway` and `edge` authorities.
- Envoy bootstraps with **glob collection locators**, not `ads: {}` wildcards:
  `lds_resources_locator` and `cds_resources_locator` carry `.../*` xdstp URLs.
- The control plane **expands globs**: a subscription whose name ends in `/*`
  matches every resource of that type sharing the xdstp prefix.
- Cross-references are xdstp too. The Listener's RDS, the route's target
  cluster, and the cluster's EDS service name are all xdstp URLs, so Envoy walks
  the whole graph by URL.

## Run it

```bash
make up
make smoke       # converge over xdstp across both authorities, then curl
make status      # print the authority map
make logs
make down
```

## What convergence looks like

```text
SUB  ty="CDS" subscribe=["xdstp://edge/envoy.config.cluster.v3.Cluster/*"]
pushing delta ty="CDS" resources=1
SUB  ty="EDS" subscribe=["xdstp://edge/envoy.config.endpoint.v3.ClusterLoadAssignment/upstream"]
pushing delta ty="EDS" resources=1
SUB  ty="LDS" subscribe=["xdstp://hardway/envoy.config.listener.v3.Listener/*"]
pushing delta ty="LDS" resources=1
SUB  ty="RDS" subscribe=["xdstp://hardway/envoy.config.route.v3.RouteConfiguration/primary"]
pushing delta ty="RDS" resources=1
```

Two things to read out of that.

**Collections are globs; references are singletons.** Envoy subscribes to LDS
and CDS with `.../*` locators (give me every member of this collection), then to
RDS and EDS with the exact xdstp URLs it found inside the Listener and Cluster.
The control plane expands the glob by prefix-matching its stored names.

**The authority is in the name.** `hardway` and `edge` are just different
prefixes on the same stream. EDS resolves under `edge` because the cluster's
`service_name` is an `edge` xdstp URL, not because a second control plane
answered.

## How the code is laid out

```text
chapter-05/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          delta server + glob-collection expansion
│   │   └── snapshot.rs      xdstp-named resources across hardway + edge
│   └── Cargo.toml
├── upstream/                unchanged from Chapter 1
├── envoy/bootstrap.yaml     lds_/cds_resources_locator with xdstp glob URLs
├── docker-compose.yml
└── Makefile
```

The only new control-plane logic over Chapter 4 is in `send_delta`: a wanted
name ending in `*` is treated as a glob and expanded to every snapshot resource
of that type whose xdstp name starts with the prefix.

## What's intentionally missing

| Missing                                            | Where it lands |
| -------------------------------------------------- | -------------- |
| Authority-to-ConfigSource mapping (real federation)| upstream Envoy |
| ORCA OOB load reports influencing LB choices       | Chapter 6      |

## Pinned versions

Same pins as Chapter 4.

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

- [Envoy xDS design notes (`source/docs/xds.md`)](https://github.com/envoyproxy/envoy/blob/main/source/docs/xds.md)
- [xDS resource names / `xdstp://` (xds.core.v3)](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol#resource-naming)
