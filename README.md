# RustGitGUI

Slint + Rustで構築されたデスクトップ向けGitクライアントです。VSCode Git Graph風のビジュアルなコミットグラフ表示と直感的なGit操作を提供します。

## 主な機能

- **📊 Git Graph表示**: ブランチ・マージを視覚的に表示
- **📝 コミット操作**: Stage/Unstage、コミット、Push/Pull
- **🌿 ブランチ管理**: 作成、切り替え、マージ、削除、コミットからブランチ作成
- **📦 Stash管理**: 作成、適用、削除
- **📥 Clone**: リモートリポジトリのクローン
- **📋 Diff表示**: ファイルごとの差分表示、Hunk単位のステージング
- **📂 リポジトリ管理**: 分類分け（グループ化）、ドラッグ&ドロップ対応

## 動作環境

- **OS**: Linux（Ubuntu等）, Windows

## インストール・起動

### 必要条件

- Rust 1.70+
- Git

### ビルド・実行

```bash
cargo run
```

### Linux環境でのPush/Pull設定（重要）

LinuxでGitHub等へのPush/Pullを行うには、**Git Credential Manager (GCM)** のインストールと設定が必要です。

#### Git Credential Managerのインストール

```bash
# 最新版のダウンロード
curl -LO https://github.com/git-ecosystem/git-credential-manager/releases/download/v2.5.0/gcm-linux_amd64.2.5.0.deb

# インストール
sudo dpkg -i gcm-linux_amd64.2.5.0.deb

# 設定
git-credential-manager configure
```

#### 認証情報の保存設定

```bash
# 保存先をsecretserviceに設定（GNOME Keyring等を利用）
git config --global credential.credentialStore secretservice

# GCMを認証ヘルパーとして設定
git config --global --replace-all credential.helper manager
```

これにより、WindowsやmacOSと同様に、ブラウザを使ったGitHub認証が可能になります。

> [!NOTE]
> Windowsでは標準でGCMがインストールされているため、この設定は不要です。

## 使い方

1. **リポジトリを開く**: 左上の📁ボタン → サイドバーから「Open Local...」
2. **コミット履歴を確認**: 中央のGraphエリアでコミット履歴を表示
3. **変更をコミット**: 「📝 Commit」ボタンでコミットモードに切り替え
4. **Push/Pull**: 上部の「⬆️ Push」「⬇️ Pull」ボタン

詳しい操作方法は [docs/FEATURES.md](docs/FEATURES.md) を参照してください。

## ドキュメント

- [機能一覧](docs/FEATURES.md) - 画面ごとの機能と操作方法
- [開発者向けガイド](docs/DEVELOPMENT.md) - ビルド方法・アーキテクチャ
- [変更履歴](CHANGELOG.md) - バージョンごとの変更内容

## ライセンス

GNU General Public License v3.0 (GPLv3)

このソフトウェアは [Slint](https://slint.dev/) フレームワークを使用しており、GPLv3ライセンス下で提供されています。
配布や改変を行う場合は、GPLv3の条項（ソースコードの開示義務など）に従ってください。
