# xDS Control Plane the Hard Way

Build an xDS control plane from raw protobuf, in the spirit of Kelsey Hightower's
*Kubernetes the Hard Way*. No Istio, no service-mesh framework. Just Envoy, the
`cncf/xds` proto family, and Rust.

The goal: peel back enough abstraction that `LDS / RDS / CDS / EDS`, `ACK/NACK`,
ADS, Delta, `xdstp://`, ORCA stop being mysterious words. Each chapter is a
single working stack you can `make up` against.

## Companion reading

These dev.to articles are the conceptual map; this repo is the lab.

- [xDS Deep Dive](https://dev.to/) ... LDS/RDS/CDS/EDS, ACK/NACK, ADS, SotW vs Delta
- [xDS Deep Dive 続編](https://dev.to/) ... `cncf/xds`, xdstp://, Dynamic Parameters, ORCA, Unified Matcher

## Stack

- **Data plane**: upstream Envoy in Docker
- **Control plane**: hand-rolled Rust binary using `tonic` + [`xds-api`](https://crates.io/crates/xds-api)
- **Upstream service**: tiny Rust HTTP server (`hyper`)
- **Glue**: `docker-compose` + `make`

`xds-api` only does code generation from proto. The ADS server, snapshot logic,
and ACK/NACK observation are written by hand on purpose.

## Chapters

| # | Title | Goal | Status |
| - | ----- | ---- | ------ |
| 01 | Hello, xDS | `curl localhost:10000` returns the upstream body, ADS logs print `SUBSCRIBE / ACK` per resource type | shipped |
| 02 | Snapshot swaps and rollback (NACK) | Push a broken Listener, observe `version_info` rollback | planned |
| 03 | Static SDS to start mTLS | Inject a cert via SDS, watch hot-rotation | planned |
| 04 | Delta xDS | Same stack, Delta protocol | planned |
| 05 | `xdstp://` + multiple authorities | Federated bootstrap, second control plane | planned |
| 06 | ORCA OOB load reports | Backend pushes synthetic CPU, LB picks the lighter pod | planned |

## Quick start

```bash
cd chapter-01
make up        # docker-compose up -d (upstream + controlplane + envoy)
make smoke     # curl http://localhost:10000/
make logs      # tail controlplane logs (see SUBSCRIBE / ACK)
make down
```

## Maintenance

`cncf/xds`, `envoyproxy/envoy`, `xds-api`, `tonic`, `prost` all move
independently. To keep chapters compiling, this repo has three layers of
drift defense:

1. **GitHub Actions cron** (`ci.yml`, weekly) reruns the smoke test
   against the pinned versions. If upstream stays still, this stays green.
2. **Dependabot** (`.github/dependabot.yml`, weekly) opens PRs for
   non-breaking bumps automatically. `tonic` / `prost` / `xds-api` are
   intentionally excluded because they have to bump in lockstep, and
   Dependabot would propose unbuildable PRs.
3. **`refresh-hardway` Claude skill** (`.claude/skills/refresh-hardway/`)
   is the manual escape valve when Dependabot's PR can't apply cleanly,
   or when we want to consume a major bump on purpose. It bumps the
   lockstepped trio, runs the full per-chapter validation, and surfaces
   any breakage.

See [`.claude/skills/refresh-hardway/SKILL.md`](./.claude/skills/refresh-hardway/SKILL.md)
for the manual procedure.

## License

MIT. See [LICENSE](./LICENSE).
