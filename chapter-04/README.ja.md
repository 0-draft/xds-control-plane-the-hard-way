# Chapter 4: Delta xDS

*[English](./README.md) | 日本語*

同じスタックを増分プロトコルで。State-of-the-World は何かが変わるたびに、その型の
リソースを全部送り直していた。Delta はバージョンが動いたリソースだけを送り、
削除は明示的に名前で伝える。ここでは `delta_aggregated_resources` を実装し、
1 リソースの変更が 1 リソースの push になることを実証する。

## Chapter 2 からの差分

- Envoy の bootstrap が `api_type: DELTA_GRPC` を使う。bootstrap の変更はこれだけ。
- **各リソースが自分のバージョンを持つ**。スナップショットは
  `type_url -> (name -> entry)` になり、スナップショット全体に 1 つのバージョンを
  押すのではなく、各エントリが自分のバージョンを知っている。
- コントロールプレーンが **Delta ADS** RPC を実装する。ストリームごとの購読
  (wildcard と名前指定) を追跡し、各クライアントが既に持っているものを
  `initial_resource_versions` で記録し、変わったリソースだけを含む
  `DeltaDiscoveryResponse` を出す。

## 動かす

```bash
make up          # 3 つのコンテナをビルドして起動
make smoke       # Delta で収束 → route だけ bump → 1 リソース push を確認
make logs        # delta の SUBSCRIBE / ACK / push を見る
make down
```

手で叩くなら:

```bash
curl -i http://localhost:10000/   # x-config-version レスポンスヘッダに注目
make bump                         # POST /bump -> RouteConfiguration だけを変更
curl -i http://localhost:10000/   # x-config-version が変わり、他は動いていない
make status
```

## 増分の見え方

route config は `x-config-version` レスポンスヘッダを持つので、bump が外から
見える。`make bump` はそのリソースだけを変える。コントロールプレーンのログでは、
収束は型ごとに 1 リソースずつ送られ、bump はちょうど 1 つを送る:

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

ここから読み取れることが 3 つ。

**wildcard と名前指定の購読は明示的。** Envoy は CDS と LDS に空の名前リスト
(wildcard、「この型のものを全部よこせ」) で subscribe し、その後 cluster と
listener から学んだ具体名で EDS と RDS に subscribe する。

**`initial_resource_versions` が再接続時の再ダウンロードを避ける仕組み。**
クライアントは既に持っている `(name, version)` の組をサーバに伝え、サーバは
まだ最新のものをスキップする。新しいストリームではこのマップは空なので、最初の
レスポンスは 1 リソースずつのフルセットになる。

**1 リソースの変更は 1 リソースの push。** bump の後、動くのは RDS だけ。CDS、
LDS、EDS はバージョンが変わっていないので無音だ。これこそ Delta の眼目であり、
smoke テストが検証しているものだ。

## コードの構成

```text
chapter-04/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          delta_aggregated_resources + ストリームごとの購読状態
│   │   └── snapshot.rs      リソースごとにバージョン管理。bump は RDS だけ触る
│   └── Cargo.toml
├── upstream/                Chapter 1 から変更なし
├── envoy/bootstrap.yaml     api_type: DELTA_GRPC (変更はこれだけ)
├── docker-compose.yml
└── Makefile
```

この章の心臓は `send_delta` だ。ある型について、欲しいリソース集合 (wildcard なら
全名前に展開、そうでなければ購読済みの名前) を求め、各リソースのバージョンを
このストリームに最後に送った版と比較し、差分と削除だけを出す。何も動いていなければ
何も送らない。それがまさにクライアント側の ACK にあたる。

## あえて省いているもの

| 省いているもの                                      | 着地する章     |
| -------------------------------------------------- | -------------- |
| 複数オーソリティ (`xdstp://`)                       | Chapter 5      |
| LB の選択に影響する ORCA OOB ロードレポート          | Chapter 6      |

## ピン留めしたバージョン

Chapter 2 と同じピン。

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

- [Envoy: Incremental xDS (Delta)](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol#incremental-xds)
- [`DeltaDiscoveryRequest` / `DeltaDiscoveryResponse`](https://www.envoyproxy.io/docs/envoy/latest/api-v3/service/discovery/v3/discovery.proto)
