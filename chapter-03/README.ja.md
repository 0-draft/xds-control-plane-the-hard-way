# Chapter 3: mTLS を始めるための静的 SDS

*[English](./README.md) | 日本語*

Listener が TLS を終端するようになり、サーバ証明書は同じ ADS ストリーム上を
独立した SDS `Secret` リソースとして流れてくる。証明書は push できるただの
リソースなので、ローテーションはスナップショット差し替えと同じ仕掛けになる。
証明書は稼働中の Listener 上で、Envoy を再起動せずに切り替わる。

## Chapter 2 からの差分

- Listener のフィルタチェーンに **TLS transport socket** が付いた。その
  `DownstreamTlsContext` は SDS シークレットを名前 (`server_cert`) で参照し、
  証明書のバイト列自体は持たない。
- コントロールプレーンが新しいリソース型 **SDS `Secret`** を ADS で配る。
  Envoy が `server_cert` に subscribe し、こちらが現在の証明書で応答する。
- `POST /rotate` が証明書を **ホットローテーション** する。新しい自己署名 leaf を
  生成し、バージョンを上げ、新しいスナップショットを push する。Envoy が稼働中の
  Listener 上で差し替える。

## ワイヤ互換性の寄り道

xds-api 0.2 は SDS の `Secret` 型は生成するが、Listener 側の
`DownstreamTlsContext` / `CommonTlsContext` ラッパーは生成しない。そこで
`controlplane/src/xtls.rs` が必要最小限の部分を prost メッセージとして手書きする。
フィールド番号は Envoy 本物の proto と一致させてある (`common_tls_context = 1`、
`tls_certificate_sds_secret_configs = 6`)。バイト列はワイヤ互換なので、Envoy は
本物の `DownstreamTlsContext` の type URL でデコードする。これぞ Hard Way の
真骨頂だ。生成された型が足りないなら、protobuf は自分でエンコードする。

## 動かす

```bash
make up          # 3 つのコンテナをビルドして起動
make smoke       # SDS 経由で TLS 配信 → 証明書をローテーション → fingerprint の変化を確認
make logs        # SUBSCRIBE / ACK / SDS push を見る
make down
```

手で叩くなら:

```bash
curl -k https://localhost:10000/      # SDS 配送の証明書で TLS 終端
make rotate                           # POST /rotate -> 稼働中の Listener に新しい証明書
make status                           # GET /status  -> advertise 中のバージョン
```

## ホットローテーションの見え方

`make smoke` はサーバ証明書の fingerprint を記録し、ローテーションし、もう一度読む:

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

コントロールプレーンのログでは、シークレットは他のリソースと同じように
ストリームに乗る。Envoy は LDS/RDS の直後に名前で subscribe する:

```text
INFO client subscribed ty="SDS" resources=["server_cert"]
INFO pushing config    ty="SDS" version=v1
INFO client accepted   kind="ACK " ty="SDS" version=v1 nonce=sds-v1
...
INFO pushing config    ty="SDS" version=v2
INFO client accepted   kind="ACK " ty="SDS" version=v2 nonce=sds-v2
```

ここから読み取れることが 2 つ。

**証明書は Listener のフィールドではなくリソース。** LDS は参照 (`server_cert` を
SDS 経由で) を運び、SDS がバイト列を運ぶ。ローテーションが触るのは SDS リソース
だけで、Listener の定義は変わらない。この分離こそ SDS の眼目で、Listener が
一切関知しないペースで証明書をローテーションできる。

**ローテーションはバージョンの更新。** `POST /rotate` は新しいスナップショットを
advertise するので、シークレットは v1 -> v2 に進み、Envoy が ACK する。新しい
TLS ハンドシェイクは新しい leaf を使う。再起動も Listener の落ちもない。

## コードの構成

```text
chapter-03/
├── controlplane/
│   ├── src/
│   │   ├── main.rs          ADS の push ループ + admin /rotate
│   │   ├── snapshot.rs      SDS Secret と TLS Listener も組むように
│   │   ├── tls.rs           自己署名証明書の生成 (rcgen)
│   │   └── xtls.rs          手書きの DownstreamTlsContext / CommonTlsContext
│   └── Cargo.toml           rcgen を追加
├── upstream/                Chapter 1 から変更なし
├── envoy/bootstrap.yaml     変更なし。シークレットは既存の ADS ストリームに乗る
├── docker-compose.yml
└── Makefile
```

## あえて省いているもの

この章はサーバ側 TLS だ。Envoy が証明書を提示し、クライアントは提示しない。
相互 TLS (2 つ目の SDS 検証コンテキストでクライアント証明書を要求・検証する) は
自然な次の一手で、演習として残しておく。

| 省いているもの                                      | 着地する章     |
| -------------------------------------------------- | -------------- |
| クライアント証明書の検証 (完全な mTLS)              | 演習           |
| SotW ではなく Delta xDS                             | Chapter 4      |
| 複数オーソリティ (`xdstp://`)                       | Chapter 5      |
| LB の選択に影響する ORCA OOB ロードレポート          | Chapter 6      |

## ピン留めしたバージョン

これまでの章と同じピンに、証明書生成用の `rcgen` を追加。Dockerfile のビルド
ステージは `build-essential` を入れている。`rcgen` の `ring` バックエンドが C を
コンパイルするためだ。

| 依存                     | ピン            |
| ------------------------ | --------------- |
| `xds-api`                | `0.2`           |
| `tonic`                  | `0.12`          |
| `prost` / `prost-types`  | `0.13`          |
| `tokio`                  | `1.41`          |
| `hyper`                  | `1.5`           |
| `rcgen`                  | `0.13`          |
| Envoy イメージ           | `v1.32-latest`  |
| Rust ツールチェーン (ビルド) | `1.96-slim`  |

## 参考

- [Envoy SDS (Secret Discovery Service)](https://www.envoyproxy.io/docs/envoy/latest/configuration/security/secret)
- [Envoy `DownstreamTlsContext`](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/transport_sockets/tls/v3/tls.proto)
- [`rcgen` on docs.rs](https://docs.rs/rcgen/)
