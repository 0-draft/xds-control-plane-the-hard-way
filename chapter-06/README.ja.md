# Chapter 6: ORCA 帯域外ロードレポート

*[English](./README.md) | 日本語*

2 つのバックエンドが合成した utilization を ORCA で報告する。軽い方が結局
トラフィックの大半を捌く。実行例では、busy なバックエンド (utilization 0.9) が
200 リクエスト中 20、idle な方 (0.1) が 180 で、きれいな 1:9 だった。

## ORCA をどこで動かすかについて正直に

元の狙いは Envoy の `client_side_weighted_round_robin` が帯域外 ORCA を直接
消費することだった。**Envoy はそれをやらない。** このポリシーは
`enable_oob_load_report` をパースするが、ホストから `OrcaLoadReport` を実際に
受け取って重みに反映する配線は
[closed as not planned](https://github.com/envoyproxy/envoy/issues/34781) だ。
設定しても素の round robin になる。50/50 で、ORCA stream も一度も開かない。

そこで帯域外 ORCA クライアントは、動かせる場所で動かす。コントロールプレーンだ。
これが ORCA の Hard Way な読み方になる。バックエンドは本物の `OpenRcaService` を
出し続け、コントロールプレーンがそこから stream する OOB エージェントになり、
utilization を EDS の重みに変換して push する。あとは Envoy の通常の重み付き
round robin がやる。Envoy が自分で ORCA を消費する日が来たら、この章の
バックエンド側はもう出来上がっている。

## Chapter 1 からの差分

- **バックエンドが 1 つの h2c ポートで 2 つを提供する**。HTTP データ
  (`x-upstream` ヘッダ付き) と `xds.service.orca.v3.OpenRcaService` gRPC だ。
  その `StreamCoreMetrics` は `application_utilization` が `ORCA_UTILIZATION` で
  固定された `OrcaLoadReport` を stream する。proto は vendor して tonic-build で
  生成する。xds-api 0.2 に ORCA 型が無いからだ。
- **コントロールプレーンが OOB ORCA クライアント**になる。各バックエンドから
  stream し、読むたびに EDS の `load_balancing_weight` を再計算し (軽い方ほど
  大きい重み)、`tokio::sync::watch` チャネルで EDS を再 push する。
- 2 つのバックエンドは普通の EDS クラスタの下にあり、Envoy のデフォルトの
  重み付き round robin が重みを尊重する。

## 動かす

```bash
make up
make smoke       # 収束 → ORCA を落ち着かせ → idle な方が勝つことを確認
make dist        # 200 リクエストを採取しバックエンドごとの分布を表示
make logs        # ORCA レポートが EDS 重みになる様子を見る
make down
```

## 見え方

コントロールプレーンのログは ORCA が重みに変わる様子を示す:

```text
INFO ORCA OOB client connected host=upstream-a
INFO ORCA OOB client connected host=upstream-b
INFO ORCA report host=upstream-b utilization=0.1
INFO ORCA report host=upstream-a utilization=0.9
INFO pushing EDS weights from ORCA version=v2 weights=["upstream-a=100", "upstream-b=900"]
INFO pushing config ty="EDS" version=v2
```

そして結果のトラフィック分布:

```text
upstream-a (busy 0.9): 20    upstream-b (idle 0.1): 180
```

ここから読み取れることが 2 つ。

**utilization が重みになる。** `weight = round((1 - utilization) * 1000)` なので、
0.9 は 100 に、0.1 は 900 になる。1:9 の重み比がそのまま 1:9 のリクエスト比に
出る。Envoy の重み付き round robin は EDS の重みで算数をしているだけだからだ。

**コントロールプレーンがループを閉じる。** 負荷を読み (ORCA クライアント)、
それに作用する (EDS push)。負荷について Envoy に見えるのは重みだけで、それこそ
xDS が引く境界だ。データプレーンが実行し、コントロールプレーンが決める。

## コードの構成

```text
chapter-06/
├── controlplane/
│   ├── proto/                ORCA proto (ここでクライアント生成)
│   ├── build.rs              tonic-build: OpenRcaService クライアント
│   └── src/
│       ├── main.rs           ORCA クライアントタスク + EDS の watch-push サーバ
│       └── snapshot.rs        EDS エンドポイントが load_balancing_weight を持つ
├── upstream/
│   ├── proto/                同じ ORCA proto (ここでサーバ生成)
│   ├── build.rs              tonic-build: OpenRcaService サーバ
│   └── src/main.rs           h2c: HTTP データ + ORCA StreamCoreMetrics
├── envoy/bootstrap.yaml      Chapter 1 から変更なし
├── docker-compose.yml        upstream-a (0.9) と upstream-b (0.1)
└── Makefile
```

## あえて省いているもの

| 省いているもの                                      | 着地する場所   |
| -------------------------------------------------- | -------------- |
| Envoy がデータプレーンで ORCA を消費                | upstream Envoy |
| in-band ORCA (レスポンストレーラ)                  | 演習           |

## ピン留めしたバージョン

Chapter 1 と同じピンに、axum (バックエンドの h2c mux) と tonic-build (ORCA の
コード生成) を追加。Dockerfile のビルドステージは protoc 用に
`protobuf-compiler` を入れている。

| 依存                     | ピン            |
| ------------------------ | --------------- |
| `xds-api`                | `0.2`           |
| `tonic` / `tonic-build`  | `0.12`          |
| `prost` / `prost-types`  | `0.13`          |
| `tokio`                  | `1.41`          |
| `axum`                   | `0.7`           |
| Envoy イメージ           | `v1.32-latest`  |
| Rust ツールチェーン (ビルド) | `1.96-slim`  |

## 参考

- [ORCA: Open Request Cost Aggregation (`xds.data.orca.v3`)](https://github.com/cncf/xds/blob/main/xds/data/orca/v3/orca_load_report.proto)
- [Envoy issue 34781: ホストからの ORCA 受信 (closed, not planned)](https://github.com/envoyproxy/envoy/issues/34781)
- [Envoy client-side weighted round robin](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/load_balancing_policies/client_side_weighted_round_robin/v3/client_side_weighted_round_robin.proto)
