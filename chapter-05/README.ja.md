# Chapter 5: xdstp:// と複数オーソリティ

*[English](./README.md) | 日本語*

すべてのリソースが、ただの文字列ではなく `xdstp://` URL で命名されるようになった。
名前は 2 つのオーソリティにまたがる。`hardway` が Listener と Route を持ち、
`edge` が Cluster と Endpoint を持つ。Envoy は Listener と Cluster の collection を
glob locator で bootstrap し、そこから RDS/EDS の singleton へとグラフをたどる。
すべて xdstp 名で。

## まずフェデレーションについて正直に

この章の元の狙いは「フェデレーション bootstrap・2 つ目のコントロールプレーン」、
つまりオーソリティ `hardway` を 1 つの cp、`edge` を別の cp で配ることだった。
**Envoy はまだそれができない。** Envoy の xDS 設計ノートより:

> We do not yet support federated configuration sources, it is assumed that a
> single ADS stream or `ConfigSource` specified parallel to the `xdstp://`
> resource locator is used. Envoy will support this in the future once a
> bootstrap based mapping from authority to `ConfigSource` is supported.

つまり現状、オーソリティは命名の構造であってルーティングの構造ではない。両方の
オーソリティは同じ ADS ストリームに解決される。この章は実在するものを作る。
xdstp:// 命名と glob collection を単一 Delta ストリームで、1 つのコントロール
プレーンが両オーソリティを serve する。Envoy が authority→source マッピングを
出した日には、`edge` を 2 つ目の cp に分けるのは bootstrap の変更であって、
コードの変更ではない。

## Chapter 4 からの差分

- リソースは `xdstp://{authority}/{proto type}/{id}` で命名され、`hardway` と
  `edge` のオーソリティにまたがる。
- Envoy は `ads: {}` の wildcard ではなく **glob collection locator** で
  bootstrap する。`lds_resources_locator` と `cds_resources_locator` が `.../*`
  の xdstp URL を持つ。
- コントロールプレーンが **glob を展開** する。名前が `/*` で終わる購読は、
  その型で xdstp プレフィックスを共有するすべてのリソースに一致する。
- 相互参照も xdstp。Listener の RDS、route のターゲット cluster、cluster の EDS
  service name はすべて xdstp URL なので、Envoy はグラフ全体を URL でたどる。

## 動かす

```bash
make up
make smoke       # 両オーソリティで xdstp 収束 → curl
make status      # オーソリティマップを表示
make logs
make down
```

## 収束の見え方

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

ここから読み取れることが 2 つ。

**collection は glob、参照は singleton。** Envoy は LDS と CDS に `.../*` locator
(この collection のメンバーを全部よこせ) で subscribe し、その後 Listener と
Cluster の中で見つけた厳密な xdstp URL で RDS と EDS に subscribe する。
コントロールプレーンは保持している名前をプレフィックス一致させて glob を展開する。

**オーソリティは名前の中にある。** `hardway` と `edge` は同じストリーム上の
違うプレフィックスにすぎない。EDS が `edge` の下に解決されるのは、cluster の
`service_name` が `edge` の xdstp URL だからであって、2 つ目のコントロール
プレーンが応答したからではない。

## コードの構成

```text
chapter-05/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          delta サーバ + glob collection の展開
│   │   └── snapshot.rs      hardway + edge にまたがる xdstp 命名リソース
│   └── Cargo.toml
├── upstream/                Chapter 1 から変更なし
├── envoy/bootstrap.yaml     xdstp glob URL の lds_/cds_resources_locator
├── docker-compose.yml
└── Makefile
```

Chapter 4 から増えたコントロールプレーンのロジックは `send_delta` だけ。`*` で
終わる wanted 名を glob として扱い、その型で xdstp 名がプレフィックスで始まる
スナップショットのリソース全部に展開する。

## あえて省いているもの

| 省いているもの                                      | 着地する場所   |
| -------------------------------------------------- | -------------- |
| authority→ConfigSource マッピング (本物のフェデレーション) | upstream Envoy |
| LB の選択に影響する ORCA OOB ロードレポート          | Chapter 6      |

## ピン留めしたバージョン

Chapter 4 と同じピン。

| 依存                     | ピン            |
| ------------------------ | --------------- |
| `xds-api`                | `0.2`           |
| `tonic`                  | `0.12`          |
| `prost` / `prost-types`  | `0.13`          |
| `tokio`                  | `1.41`          |
| `hyper`                  | `1.5`           |
| Envoy イメージ           | `v1.32-latest`  |
| Rust ツールチェーン (ビルド) | `1.96-slim`  |

## 参考

- [Envoy xDS design notes (`source/docs/xds.md`)](https://github.com/envoyproxy/envoy/blob/main/source/docs/xds.md)
- [xDS resource names / `xdstp://` (xds.core.v3)](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol#resource-naming)
