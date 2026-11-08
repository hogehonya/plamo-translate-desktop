# Firefox Android対応

このフォークは、Android版Firefoxで端末内翻訳を使うための開発版です。
要求元はhogehonya、2026-11-08の依頼「androidでつかいたい」「hogehonyaにフォークしてすすめて」。
要求・残作業の追跡先は[Issue #1](https://github.com/hogehonya/plamo-translate-desktop/issues/1)です。

## 使い方

1. Mozilla署名済みのFirefox用XPIをインストールします。現在のビルド出力は**未署名ZIP**です。
2. 翻訳したいページを開き、Firefoxの拡張機能メニューからPLaMoを選びます。
3. 翻訳画面が別タブで開きます。「設定」で0.8Bモデル（573,123,200 bytes）をダウンロードします。
   初回は[PLaMo Community License](https://plamo.preferredai.jp/info/plamo-community-license-ja)への同意が必要です。
4. 文章を入力・貼り付けて翻訳するか、「このページを翻訳」を押して元ページへ戻ります。
5. 「解放」でモデルの実行メモリを解放できます。保存済みのモデルは残り、次の翻訳時に再読み込みします。

Firefox用はCPU単一スレッド、同時翻訳数1です。2Bも選択できますが、まず0.8Bを使ってください。
モデルファイル以外に推論メモリと保存用の空き容量が必要です。ダウンロード確定時には一時ファイルと
最終ファイルが併存するため、少なくともモデルサイズの2倍に加えて余裕を確保してください。
端末のメモリ不足やAndroidによるバックグラウンドプロセス終了では翻訳が中断する可能性があります。
翻訳中はFirefoxを前面で使い、中断時はPLaMoを開き直してください。

ページ翻訳は呼び出し元のタブを保持します。そのタブを閉じるとボタンは無効になります。
別サイトに遷移した場合は、そのページからPLaMoを開き直してください。
Firefoxの内部ページや拡張機能ストアなど、ブラウザが注入を禁止するページは翻訳できません。
翻訳本文は端末内で処理します。モデル取得時にはHugging Faceへ接続します。

## ビルドとインストール

Python 3とNode.js 22以降を使います。追加のビルド依存は不要です。

```sh
python3 scripts/package-firefox.py
```

出力は`dist/plamo-translate-VERSION-firefox-unsigned.zip`です。
Chrome / Edgeは従来どおり`extension/manifest.json`を使用します。
Firefox用は`extension/manifest.firefox.json`を適用し、Workerを事前生成して同梱します。
**Firefoxではソースのextensionフォルダを直接読み込まず、ビルドしたZIPを使用してください。**

PC版Firefoxの開発確認では`about:debugging` →「このFirefox」→「一時的なアドオンを読み込む」からZIPを選択できます。
通常版Firefoxへの恒久インストールにはMozilla署名が必要です。未署名ZIPをXPIにリネームするだけでは導入できません。
署名にはAMO開発者アカウントが必要です。公開配布または自己配布の手順は
[Mozillaの署名・配布説明](https://extensionworkshop.com/documentation/publish/signing-and-distribution-overview/)に従ってください。
Androidへの署名済みXPIの導入手順は
[自己配布拡張のインストール](https://extensionworkshop.com/documentation/publish/install-self-distributed/)を参照してください。
このリポジトリのCIは未署名ZIPを成果物として保存します。自動署名・AMO公開は行いません。

## 変更内容

- Firefoxのイベントページ内でエンジン用iframeを起動・終了。起動失敗と起動中の解放も処理。
- 拡張ボタンから通常タブを開き、URLに元タブIDを保持。サイドバー・コンテキストメニュー・Native Messagingには依存しない。
- FirefoxではBlob WorkerがCSPに拒否されるため、同梱wllamaのコードから静的Workerを生成。
  元のwllamaファイルは変更せず、Firefoxパッケージ内だけに変換結果を入れる。
  生成元の構造が変わった場合はビルドを失敗させる。ブラウザのセキュリティ設定は変更しない。
- Firefox未実装のOPFS `move()`の代わりにストリームコピーでモデル保存を確定。
  コピー失敗時には再開用の一時ファイルを残す。Chromeの既存`move()`は維持。
- Firefox用タッチ操作サイズ、日本語・英語の案内、0.8B優先表示。

## 検証

```sh
node tests/extension/translator.test.mjs
node tests/extension/firefox.test.mjs
node tests/extension/model-storage.test.mjs
python3 scripts/package-firefox.py
# ZIPを専用の一時フォルダに展開してweb-extで検査
python3 -m zipfile -e dist/plamo-translate-0.4.0-firefox-unsigned.zip dist/firefox
npx --yes web-ext@10.8.0 lint --source-dir dist/firefox
```

実ブラウザの統合テストはPythonのSeleniumとFirefoxを別途用意して実行します。
専用の一時プロファイルを作成し、約573 MBを取得します。ユーザーの通常プロファイルは使いません。
モデルライセンスに同意した場合のみ次を実行してください。

```sh
SE_AVOID_STATS=true python3 tests/extension/firefox-smoke.py --accept-model-license
```

2026-11-08の確認結果:

- JavaScript構文検査、既存翻訳テスト、Firefox起動・元タブ引継ぎ・エンジン接続元検証・起動キャンセル・再起動テスト、保存失敗回復テスト: 成功。
- web-ext 10.8.0: エラー0件。Chrome専用分岐の`offscreen`/`sidePanel`参照に対する静的警告5件。
  Firefox分岐では呼び出さず、Chrome APIを持たないテスト環境でも起動確認済み。
- Linux Firefox 153.6.0 ESR: 一時インストール、OPFS書き込み、実モデルの取得、
  `The sky is blue.` → `空は青い。`、解放後の再読み込みと再翻訳に成功。
- 元ページ翻訳: 呼び出し元タブで`The sky is blue. The sun is bright today.`が
  `空は青い。今日は太陽が明るい。`に置換されることを実ブラウザで確認。
  テストでは元ページの読み込み完了後に、拡張ボタンと同じactiveTab権限を付与。
- manifestの最低バージョンは142。最低バージョンそのものとAndroid実機の動作は未確認。
- Android実機、Mozilla署名、GitHub CI・統合は未完了。デスクトップ版の成功をAndroidの成功とは扱わない。
- 独立レビューは未実施。GitHubを扱えない既存impl経路に対し、ユーザーが親エージェントの直接実装を許可。

## 作業checkpoint

1. フォーク・Issue作成: 完了。正本はhogehonya/plamo-translate-desktop、Issue #1。
2. Firefoxパッケージ実装とローカル検証: 完了。短文翻訳・解放と再読み込み・元ページ翻訳まで確認済み。
3. 実機確認と統合: 未完了。`.20`のUSB端末確認には、r5600xから`.20`への既存SSH認証が必要。
   `honya@192.168.10.20`は公開鍵提示後に拒否された。認証設定は変更していない。
