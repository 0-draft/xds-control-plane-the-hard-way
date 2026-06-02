# Chapter 2: スナップショットの差し替えとロールバック

*[English](./README.md) | 日本語*

Chapter 1 は固定された 1 つのスナップショットを配るだけだった。ここでは
コントロールプレーンが実行中に方針を変え、新しいバージョンを Envoy に
（要求を待たずに）push し、Envoy が拒否したらロールバックする。

## Chapter 1 からの差分

- スナップショットが **バージョン付きで可変** になった。`:19000` の admin HTTP
  API が新しいスナップショットを組み立てて advertise する。
- ADS ストリームが **サーバ起点の push** をする。`tokio::sync::watch` チャネルで
  スナップショットの変更を接続中の Envoy に流す。要求に答えるだけではない。
- **NACK 時のロールバック**。Envoy が push された設定を拒否したら、
  コントロールプレーンは last-known-good のスナップショットを advertise し直し、
  `:10000` は配信を止めない。

## 何を作るか

```text
                       :19000 (admin HTTP)
   operator ──POST /push/broken──►  controlplane (Rust)
                                         │  watch チャネル
              :10000 (HTTP)             ▼
   curl  ───────────────►  Envoy  ◄── :18000 (gRPC ADS) ── v2 push / NACK / v1 へロールバック
                              │
                              ▼
                          upstream (:9000)
```

## 動かす

```bash
make up          # 3 つのコンテナをビルドして起動
make smoke       # v1 に収束 → 壊れた Listener を push → ロールバックを確認 → good に差し替え
make logs        # SUBSCRIBE / ACK / NACK / rollback を見る
make down
```

admin API を手で叩くなら:

```bash
make push-broken   # POST /push/broken  -> Envoy が拒否する Listener を advertise
make push-good     # POST /push/good    -> クリーンなバージョン差し替え
make status        # GET  /status       -> advertised と last_good のバージョン
```

## ロールバックの見え方

`make push-broken` は `HttpConnectionManager` の `stat_prefix` が空の Listener を
advertise する。スナップショットの他の部分はすべて妥当なので、NACK はその
Listener 1 つに対するものだと一意に分かる。コントロールプレーンのログ:

```text
INFO pushing config ty="EDS" version=v2
INFO pushing config ty="RDS" version=v2
INFO pushing config ty="LDS" version=v2
INFO pushing config ty="CDS" version=v2
INFO client accepted config kind="ACK " ty="EDS" version=v2 nonce=eds-v2
INFO client accepted config kind="ACK " ty="RDS" version=v2 nonce=rds-v2
WARN client rejected config kind="NACK" ty="LDS" version=v1 nonce=lds-v2 msg=Error adding/updating listener(s) primary_listener: Proto constraint validation failed (HttpConnectionManagerValidationError.StatPrefix: value length must be at least 1 characters)
INFO rolling back after NACK rollback_to=v1
INFO client accepted config kind="ACK " ty="CDS" version=v2 nonce=cds-v2
INFO pushing config ty="LDS" version=v1
INFO client accepted config kind="ACK " ty="LDS" version=v1 nonce=lds-v1
```

このトレースから読み取れることが 3 つ。

**1 つのバージョンが 4 つのリソース型すべてにまたがる。** v2 を push すると
CDS / EDS / RDS / LDS の新しいレスポンスが送られる。妥当な 3 つは ACK され、
Listener だけが失敗する。

**NACK は Envoy 自身の検証エラーを返してくる。** NACK の `version=v1` は、
Envoy が LDS について実際に最後に受け入れたバージョンを伝えている（いま拒否した
バージョンではない）。`msg` は proto 検証の失敗そのままで、本物の壊れた push を
デバッグするときに見るのはこれだ。

**ロールバックは Envoy ではなくコントロールプレーンの仕事。** Envoy は NACK の
とき最後に妥当だった Listener を保持するので、トラフィックは落ちない。だが
*advertise されている* バージョンは、コントロールプレーンが v1 を advertise し
直すまで壊れた v2 のまま。その advertise し直しがロールバックであり、
`last_good` が存在する理由だ。

## コードの構成

```text
chapter-02/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          ADS の push ループ (watch)、admin API、ロールバック
│   │   └── snapshot.rs      good() と broken() のスナップショットビルダー
│   └── Cargo.toml
├── upstream/                Chapter 1 から変更なし
├── envoy/bootstrap.yaml     Chapter 1 から変更なし
├── docker-compose.yml       controlplane が :19000 も公開するように
└── Makefile
```

コントロールプレーンは Chapter 1 になかったものを 2 つ持つ。現在 advertise して
いるスナップショットのための `watch::Sender<Arc<Snapshot>>` と、`last_good`
スロットだ。`push_broken` は `last_good` を触らずに advertise し、`push_good` は
両方を更新する。NACK のときストリームは `last_good` を advertise し直し、
`select!` ループの watch アームがそれを再 push する。

## あえて省いているもの

| 省いているもの                                      | 着地する章     |
| -------------------------------------------------- | -------------- |
| mTLS 用の SDS マテリアル                            | Chapter 3      |
| SotW ではなく Delta xDS                             | Chapter 4      |
| 複数オーソリティ (`xdstp://`)                       | Chapter 5      |
| LB の選択に影響する ORCA OOB ロードレポート          | Chapter 6      |

## ピン留めしたバージョン

Chapter 1 と同じピン。`tonic`、`prost`、`xds-api` はロックステップで動くので、
この 3 つはまとめてバンプして再ビルドする。1 つずつは触らない。

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

- [Envoy xDS protocol: ACK/NACK and resource warming](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol#ack-nack-and-versioning)
- [tokio `watch` channel](https://docs.rs/tokio/latest/tokio/sync/watch/)
