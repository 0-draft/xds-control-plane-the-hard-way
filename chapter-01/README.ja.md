# Chapter 1: Hello, xDS

*[English](./README.md) | 日本語*

「xDS コントロールプレーン」と名乗れる、ぎりぎり最小のもの。

## 何を作るか

```text
              :10000 (HTTP)
   curl  ─────────────────►  Envoy  ◄────── :18000 (gRPC ADS) ──── controlplane (Rust)
                                ▲                                       │
                                │       LDS / RDS / CDS / EDS over ADS  │
                                │                                       │
                                ▼                                       ▼
                            upstream (Rust hyper)  on  :9000  ◄───────  (snapshot v1)
```

- **upstream** : `:9000` で `hello from <hostname>` を返す小さな `hyper` サーバ
- **controlplane** : スナップショット `v1` を 1 つだけ保持する `tonic` ADS サーバ。Listener / Route / Cluster / Endpoint を 1 つずつ、`xds-api` の proto 型から手作業で組み立てている
- **envoy** : 公式 Envoy イメージ。`bootstrap.yaml` を読み、唯一の静的クラスタ経由で controlplane を見つけ、残りは動的に取得する

スタックが収束すると、こうなる。

```bash
$ curl http://localhost:10000/
hello from <upstream-container-id>
path: /
method: GET
```

## 動かす

```bash
make up          # 3 つのコンテナをビルドして起動
make smoke       # Envoy の収束を待ってから curl
make logs        # controlplane が出力する SUBSCRIBE / ACK を見る
make down
```

プロトコルの姿が見えるのは controlplane のログだ。

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

このトレースで注目すべき点が 2 つある。

**依存グラフのたどり方が見えている。** Envoy はまず `CDS` と `LDS` に空の
`resource_names` で subscribe する。これはワイルドカード形式
(「持っているものを全部よこせ」) だ。レスポンスにはクラスタ
`upstream_cluster` とリスナー `primary_listener` が載る。Envoy はそれらを読み、
クラスタが `EDS` を、リスナーが `RDS` を使うと分かると、`upstream_cluster` と
`primary_route` という 2 つの具体名に subscribe する。続いて 4 つの ACK が来る。

**`node=` フィールドが埋まるのは最初のリクエストだけ。** これは
`envoy/bootstrap.yaml` の `set_node_on_first_message_only: true` のノブによる。
Envoy はストリームの最初に完全な `Node` を 1 度だけ送り、同じストリーム内の
以降のメッセージでは省略する。コントロールプレーン側がそれを覚えておく必要がある。

## コードの構成

```text
chapter-01/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          ADS サーバ実装 + ACK/NACK ロギング
│   │   └── snapshot.rs      xds-api 型から手作業で組んだ v1 スナップショット
│   └── Cargo.toml
├── upstream/
│   ├── src/main.rs          :9000 の hyper HTTP サーバ
│   └── Cargo.toml
├── envoy/
│   └── bootstrap.yaml       Envoy 設定 (静的クラスタは controlplane の 1 つだけ)
├── docker-compose.yml
└── Makefile
```

## あえて省いているもの

Chapter 2 以降を読む理由を残すために。

| 省いているもの                                      | 着地する章                      |
| -------------------------------------------------- | ------------------------------- |
| 可変スナップショット。v1 は永遠にハードコード        | Chapter 2                       |
| 本物の NACK デモ (壊れた設定を push してロールバックを観測) | Chapter 2                |
| mTLS 用の SDS マテリアル                            | Chapter 3                       |
| SotW ではなく Delta xDS                             | Chapter 4                       |
| 複数オーソリティ (`xdstp://`)                       | Chapter 5                       |
| LB の選択に影響する ORCA OOB ロードレポート          | Chapter 6                       |

## ピン留めしたバージョン

これらは意図的なピン留め。`tonic`、`prost`、`xds-api` はロックステップで
動くので、この 3 つはまとめてバンプして再ビルドする。1 つずつは触らない。

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

- [xds-api on docs.rs](https://docs.rs/xds-api/)
- [Envoy xDS REST and gRPC Protocol](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol)
- [Envoy bootstrap config reference](https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/bootstrap/v3/bootstrap.proto)
