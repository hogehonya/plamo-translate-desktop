# PLaMo Translate Desktop

[English](README.md)

[PLaMo 3 Translate](https://www.preferred.jp/ja/blog/tech/plamo-3-translate-weight-release) を手元のパソコンで動かす翻訳デスクトップアプリです。
モデルをダウンロードした後は、オフラインで使えます。Rust と [GPUI](https://www.gpui.rs/) で実装しています。

**Built with PLaMo.**

## 機能

- 53 言語の翻訳。翻訳先の既定は「日本語 ⇄ 英語（自動）」です。
- 貼り付けるとすぐに翻訳し、入力中の文章は手が止まってから少し待って翻訳します（ツールバーの「自動翻訳」でオフにできます）。
  訳文は生成と同時に表示され、ステータスバーに進み具合、速度、残り時間が出ます。途中で止めることもできます。
- 日本語の文体（敬体・常体）の指定、参考情報としての文脈、用語集（1 行に 1 語、`原語 = 訳語`）。
- 長い文章は段落ごとに分けて順に訳します。直前の原文と訳文を履歴として渡すので、訳語や文体がそろいます。
  訳文が同じ言葉の繰り返しに陥ったときは、それを検出して取り消し、小さく分けて訳し直します。それでも繰り返す箇所は打ち切り、訳文に印を付けます。
- モデル管理機能。サイズ（0.8B、2B、8B、31B）と量子化の種類を選べます。
  - 各ファイルに、PFN が公表している翻訳精度と、量子化なしのモデルとの差を表示します。
  - ダウンロードは中断しても続きから再開でき、SHA-256 で検証します。
  - 新しく公開されたファイルは、アプリを更新しなくても Hugging Face から自動で一覧に加わります。
- CPU（AVX2 または AVX-512）でも、Vulkan 経由の GPU でも動きます。Linux でソースからビルドする場合は ROCm も使えます。
  実行デバイスは選択でき、自動選択では単体 GPU を選びます。
- 英語と日本語の UI。既定では OS の言語に合わせます。ステータスバーの歯車から開く「設定」で変更でき、環境変数 `PLAMO_LANG=ja` または `en` を指定するとどちらよりも優先されます。
- ライトとダークのテーマ。既定では OS の設定に合わせ、「設定」でどちらかに固定することもできます。
- 原文と訳文の連動スクロール。片方でカーソルを動かすと、もう片方で対応する段落を選択します。「設定」でオフにできます。

ショートカット: `Ctrl+Enter` ですぐに翻訳、`Ctrl+Shift+C` で訳文をコピー。

## モデルとライセンス

アプリにはモデルの重みを同梱していません。アプリでモデルを選ぶと、[Hugging Face の Preferred Networks のページ](https://huggingface.co/pfnet)からダウンロードします。
モデルには [PLaMo コミュニティライセンス](https://plamo.preferredai.jp/info/plamo-community-license-ja)（[英語版](https://plamo.preferredai.jp/info/plamo-community-license-en)）が適用されます。アプリは最初のダウンロードの前に同意を求めます。

- **個人利用と非商用利用**は無料です。
- **商用利用**（組織内での利用を含む）では、利用者（または関連会社）の年間売上が 10 億円以下なら[事前登録](https://forms.gle/mTL8tBLrMYXKNZD56)が必要です。
  10 億円を超える場合は、Preferred Networks との別途の契約が必要です。
- モデルで翻訳した結果を公開するときは、PLaMo の出力であることを明示してください（第 5 条）。

PLaMo は株式会社 Preferred Networks の商標です。

## ダウンロード

Windows（x86_64）版と Linux（x86_64）版は [Releases](https://github.com/tanakh/plamo-translate-desktop/releases) からダウンロードできます。
SHA-256 のチェックサムは `SHA256SUMS.txt` にあります。最新の変更を試すときは、ソースからビルドしてください（後述）。

- **Windows**（10 以降）: 展開して `plamo-translate-desktop.exe` を実行します。同梱の DLL は exe と同じフォルダに置いたままにしてください。
  llama.cpp 本体と、CPU・Vulkan のバックエンドはそこから読み込み、それ以外の場所からは読み込みません。
  Visual C++ ランタイムも同梱しているので、ほかに何かをインストールする必要はありません。
- **Linux**: 展開して `plamo-translate-desktop` を実行します。同梱の共有ライブラリは、実行ファイル自身のフォルダ（`RUNPATH=$ORIGIN`）から読み込みます。
  - Ubuntu 24.04 でビルドしているので、glibc 2.39 以降（Ubuntu 24.04、Debian 13、Fedora 40 以降）が必要です。
    ほかに `libgomp1`、`libxkbcommon-x11-0`、`libxcb1` が必要ですが、デスクトップ環境なら通常は入っています。
  - Vulkan ドライバの入った Wayland または X11 のデスクトップ環境が必要です。Vulkan は UI の描画に使い、GPU での翻訳にも使います。
    また、`fonts-noto-cjk` などの CJK フォントが必要です。
  - アプリケーションメニューに登録するには、展開したフォルダで次を実行します。

    ```sh
    mkdir -p ~/.local/share/applications ~/.local/share/icons/hicolor/256x256/apps
    sed "s|^Exec=.*|Exec=\"$PWD/plamo-translate-desktop\"|" plamo-translate-desktop.desktop \
      > ~/.local/share/applications/plamo-translate-desktop.desktop
    cp plamo-translate-desktop.png ~/.local/share/icons/hicolor/256x256/apps/
    ```

どちらも、Vulkan で使える単体 GPU があればそれで翻訳し、無ければ CPU が対応する最速の方式（AVX2 または AVX-512）で翻訳します。

## ブラウザ拡張

配布物には、Chrome と Edge 用の拡張機能（`browser-extension/`）が入っています。アプリで選んだモデルで、ブラウザ上の文章を翻訳できます。

1. アプリのステータスバーの歯車から「設定」を開き、「ブラウザ拡張」の **連携する** を押します。
   同梱の `plamo-translate-host` を Chrome と Edge に登録します（Linux では Chromium と Brave にも登録します）。
2. `chrome://extensions`（Edge なら `edge://extensions`）を開いてデベロッパー モードをオンにし、**パッケージ化されていない拡張機能を読み込む** で
   `browser-extension` フォルダを選びます（同じ設定の **拡張機能のフォルダを開く** で開けます）。
3. ページの文章を選択して右クリックし、**PLaMo で翻訳** を選ぶと、サイドパネルに訳文が流れるように表示されます。
   ツールバーのボタンでパネルを開き、文章を入力したり貼り付けたりして訳すこともできます。

パネルを開いている間、ブラウザがホストを起動します。ホストは、パネルの **モデルとデバイス** で別のものを選ばない限り、
アプリで選んだモデルとデバイスを使います（選べるのはダウンロード済みのモデルで、ダウンロードはアプリで行います）。
モデルは別に読み込むので、アプリも開いているとメモリにモデルが 2 つ載ります。アプリのフォルダを移動したときは、もう一度「連携する」を押してください。
Linux の Snap 版や Flatpak 版のブラウザは、ホストを起動できません。

## ソースからのビルド

必要なもの: Rust 1.92 以上、CMake、C/C++ コンパイラ。

Linux（Ubuntu または Debian）:

```sh
sudo apt install cmake libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev
cargo run -p plamo-translate-desktop --release
```

この開発用のビルドは CPU で動き、llama.cpp を静的にリンクします。
配布用のパッケージは Release ワークフローと同じ方法でビルドします。サイズを抑えた `dist` プロファイル（fat LTO、シンボル除去）を使い、Vulkan と実行時に読み込むバックエンドを有効にします。
Vulkan のバックエンドには、さらに `libvulkan-dev glslc spirv-headers` が必要です。

```sh
cargo build --profile dist -p plamo-translate-desktop --features engine/vulkan,engine/dynamic-backends
```

AMD GPU を積んだ Linux（WSL2 を含む）では、ROCm（HIP）のバックエンドで GPU を使えます。
ROCm と開発用パッケージが必要です。RX 9070 XT なら、たとえば `amdrocm-core-dev10.1-gfx1201 amdrocm-blas-dev10.1` です。
`scripts/rocm.sh` が環境を整え、`target/rocm` にビルドします。

```sh
scripts/rocm.sh cargo run -p plamo-translate-desktop --release --features rocm
```

Windows では、Visual Studio 2022 の「C++ によるデスクトップ開発」（MSVC と Windows SDK）と CMake が必要です。
GPU 版のビルドには、さらに [Vulkan SDK](https://vulkan.lunarg.com/sdk/home#windows) が必要です。

```powershell
cargo build -p plamo-translate-desktop --release --features engine/vulkan,engine/dynamic-backends
```

`dynamic-backends` を有効にした場合は、`target/release/build/llama-cpp-sys-2-*/out/backends` にある `ggml-*.dll` を実行ファイルと同じフォルダに置くか、`cargo run` で起動してください。`cargo run` ならビルドフォルダから見つけます。

### 開発

- `crates/pfml`: PFML3（PLaMo のプロンプト形式）のプロンプト構築。
- `crates/engine`: llama.cpp のバックエンド、モデルカタログ、ダウンロード、長文の分割。GUI には依存しません。
- `crates/appdata`: 設定、ファイルの置き場所、ログ。アプリとブラウザ用のホストで共有します。
- `crates/app`: GPUI のアプリケーション本体。
- `crates/host`: ブラウザ拡張用の Native Messaging のホスト（`plamo-translate-host`）。
- `extension/`: ブラウザ拡張（Manifest V3）。
- `docs/design.md`: 設計書（英語）。

GUI なしでエンジンを動かすコマンドラインの例が 2 つあります。

```sh
cargo run -p engine --release --example translate -- --model path/to/model.gguf --to en "こんにちは"
cargo run -p engine --release --example download -- --list
```

第 1 引数（または環境変数 `PLAMO_MODEL`）に GGUF ファイルのパスを渡すと、アプリで選んだモデルの代わりにそれを使います。

### ファイルとログ

モデル（`models/` の下）、モデル一覧のキャッシュ（`catalog.json`）、小さなログ（`app.log`）は次の場所に保存します。

- Windows: `%LOCALAPPDATA%\plamo-translate-desktop\data`
- Linux: `~/.local/share/plamo-translate-desktop`（`$XDG_DATA_HOME`）

ログには、起動、検出したデバイス、モデルの読み込み、ダウンロード、削除、設定の変更、長い文章のやり直しや打ち切りの位置を記録します。翻訳した文章は記録しません。
起動時に 1 MiB を超えていたら `app.log.1` に退避します。

設定は `settings.json` に保存します。場所は、Windows では `%APPDATA%\plamo-translate-desktop\config`、Linux では `~/.config/plamo-translate-desktop`（`$XDG_CONFIG_HOME`）です。

## ライセンス

このアプリのソースコードは、次のどちらかのライセンスで利用できます。どちらを選ぶかは利用者が決めてください。

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT License（[LICENSE-MIT](LICENSE-MIT)）

PLaMo のモデルはこのリポジトリに含まれず、PLaMo コミュニティライセンスが適用されます。

配布物にはサードパーティーのソフトウェア（Rust のクレート、llama.cpp、Lucide のアイコン、Windows 版では Visual C++ ランタイム）が含まれます。
それらのライセンスは、各配布物の `THIRD-PARTY-NOTICES.txt` にあります（`scripts/notices.sh` で生成）。
