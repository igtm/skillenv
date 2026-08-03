---
name: plan-visualizer
description: 実行計画やアーキテクチャ設計を、ブラウザで閲覧可能な1枚のリッチなHTMLファイル（Tailwind CSS + Mermaid.js）として可視化する。user-context スキルから渡された Visual Preferences を厳格に適用する。
---

# Plan Visualizer

## 1. 概要

このスキルは、考案した実行計画やアーキテクチャ設計を、ブラウザで閲覧可能な1枚のリッチなHTMLファイルとして可視化する。

- 使用技術: Tailwind CSS（CDN） + Mermaid.js（CDN）
- `user-context` スキルから渡されたユーザーの `Visual Preferences`（UI/デザインの好み）を厳格に適用する。
- 設定が存在しない場合は、サンプルの計画書を生成してユーザーに提示し、視覚的に好みをヒアリングする。

## 2. 前提条件

このスキルを実行する前に、**必ず `user-context` スキルが完了し、現在の実行ユーザーのプロファイルがロードされている必要がある**。

`user-context` が完了していない場合は、まず `user-context` を実行してからこのスキルに戻る。

## 3. 実行ワークフロー

### Step 1: ユーザー設定の検証とサンプル提示（ヒアリングフェーズ）

1. コンテキストにロードされたユーザープロファイルの `Visual Preferences`（テーマ色、アクセントカラー、詳細説明のUI配置の好みなど）を確認する。
2. 設定が不足している、または空の場合:
   - 汎用的なダミーの実行計画（例: 「APIサーバーの構築フロー」）を、Mermaid図と補足説明を含む本格的なサンプルとして生成する。
   - 後述の Step 3〜4 の手順に従い、サンプルのHTMLファイルを保存してブラウザで起動する。
   - ブラウザ起動後、ユーザーに以下のように尋ねる:
     > 「サンプルの計画書をブラウザで開きました。配色の好み（ダーク/ライト、特定の色など）や、補足説明の見せ方（例: グリッドカード、シンプルなリストなど）について、ご希望を教えてください。」
   - ユーザーからの回答を受け取り、好みを把握した上で Step 2 へ進む。
   - **把握した好みは自律的にユーザープロファイル（`.agents/users/${user_name}.md`）の `Visual Preferences` セクションへ追記しておく。**

### Step 2: 計画の構造化とHTMLの生成

1. ユーザーからの本来の要求（例：「認証基盤の移行計画を図解して」）を解析し、ステップと依存関係を抽出する。
2. 適用されたプロファイル（または Step 1 でヒアリングした好み）に基づき、計画内容を Mermaid 記法（`graph TD` 等）で図解する。
3. `classDef` を用いたノードの配色や、図の周辺のUI（Tailwind CSS）を好みに合わせてカスタマイズし、1枚の独立したHTMLを生成する。
   - 詳細は後述の「ベースHTMLテンプレート」を参照。

### Step 3: 出力パスの決定とファイルの保存

1. 計画の出力ごとに一意のファイルを生成し、上書き衝突を防ぐ。
2. 現在の作業ディレクトリ名、または Git リポジトリ名から `${project_name}` を取得する。
3. 以下の規則に従い、プロジェクトごとのディレクトリにタイムスタンプ付きのファイルとしてHTMLを書き出す。
   - 保存先パスのフォーマット: `/tmp/agents_plans/${project_name}/plan_${YYYYMMDD_HHMMSS}.html`
   - 例: `/tmp/agents_plans/auth-api/plan_20260619_143000.html`

### Step 4: プレビューの自動起動

ファイルの保存完了後、ローカルマシンの標準コマンドを使用して、ブラウザで生成したプレビューを自動的に立ち上げる。

- macOS: `open <保存したファイルのフルパス>`
- Linux: `xdg-open <保存したファイルのフルパス>`
- Windows: `start <保存したファイルのフルパス>`

## 4. Visual Preferences の適用ルール

`user-context` から取得した `Visual Preferences` を以下のようにHTMLへ反映する。

| 設定項目 | 適用先 |
|---|---|
| テーマ（ダーク/ライト） | `<body>` の背景色・文字色、`mermaid.themeVariables` |
| アクセントカラー | ヘッダーの下線、ノードの `classDef`、リンク、強調要素 |
| UI形状（シャープ/ラウンド） | カードやコンテナの `rounded-*` クラス |
| 補足説明の配置（カード/リスト） | ステップ詳細のレイアウト（`grid grid-cols-2` / `flex flex-col` など） |

### デフォルト値

`Visual Preferences` が不足している場合のデフォルト:

- テーマ: ダーク (`bg-slate-900 text-slate-100`)
- アクセントカラー: `#00ffcc`
- UI形状: シャープ (`rounded-none`)
- 補足説明: グリッドカード (`grid grid-cols-1 md:grid-cols-2 gap-4`)

## 5. ベースHTMLテンプレート

出力時は以下の構造を維持し、`<!-- DYNAMIC: ... -->` コメントの部分を動的に生成・置換すること。

```html
<!DOCTYPE html>
<html lang="ja">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>AI Execution Plan - <!-- DYNAMIC: plan_title --></title>
  <script src="https://cdn.tailwindcss.com"></script>
  <script>
    tailwind.config = {
      theme: { extend: { colors: { brand: '<!-- DYNAMIC: accent_color -->' } } }
    }
  </script>
</head>
<body class="min-h-screen p-8 font-sans <!-- DYNAMIC: theme_classes -->">
  <div class="max-w-4xl mx-auto">
    <header class="mb-8 pb-4 border-b border-opacity-30 border-current">
      <h1 class="text-3xl font-bold mb-2"><!-- DYNAMIC: plan_title --></h1>
      <p class="text-sm opacity-70">
        Project: <!-- DYNAMIC: project_name --> | Generated at: <!-- DYNAMIC: generated_at -->
      </p>
    </header>

    <div class="mb-8 overflow-x-auto">
      <pre class="mermaid flex justify-center">
        <!-- DYNAMIC: mermaid_diagram -->
      </pre>
    </div>

    <div class="<!-- DYNAMIC: details_layout_classes -->">
      <!-- DYNAMIC: step_detail_cards -->
    </div>
  </div>

  <script type="module">
    import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.esm.min.mjs';
    mermaid.initialize({
      startOnLoad: true,
      theme: 'base',
      themeVariables: {
        background: 'transparent',
        primaryColor: '<!-- DYNAMIC: primary_color -->',
        primaryTextColor: '<!-- DYNAMIC: text_color -->',
        primaryBorderColor: '<!-- DYNAMIC: border_color -->',
        lineColor: '<!-- DYNAMIC: line_color -->'
      }
    });
  </script>
</body>
</html>
```

### 動的置換項目の詳細

| プレースホルダ | 内容 |
|---|---|
| `plan_title` | 計画のタイトル。例: 「認証基盤移行計画」 |
| `project_name` | 現在のプロジェクト名 |
| `generated_at` | 生成日時。例: `2026-06-19 14:30:00` |
| `accent_color` | Tailwind用ブランドカラー。例: `#00ffcc` |
| `theme_classes` | テーマに応じた body クラス。例: `bg-slate-900 text-slate-100` |
| `mermaid_diagram` | `graph TD` などのMermaid記法 |
| `details_layout_classes` | 詳細セクションのTailwindレイアウトクラス |
| `step_detail_cards` | 各ステップの説明カードまたはリスト要素 |
| `primary_color` / `text_color` / `border_color` / `line_color` | Mermaid テーマ変数用の色 |

## 6. 出力フォーマット制約

- **外部依存の制限**: HTMLにはCDN経由の Tailwind CSS と Mermaid.js のみを含め、ローカルのCSS/JSファイルには依存させないこと。
- **動的レイアウトの適応**: Step 1 で「カード形式にして」「ステップごとに枠線で囲って」などのUIに関する要望を受けていた場合、Step 2 のHTML生成時に Tailwind CSS のクラスを駆使して柔軟にレイアウトを変更すること。
- **単一ファイル**: すべてのスタイルとスクリプトを1枚のHTMLファイルに内包すること。

## 7. サンプル計画書（ヒアリング用）

`Visual Preferences` が未設定の場合に生成するサンプル内容の例。実際には `assets/sample-plan.html` を参考にして、動的に同様の内容を生成する。

- タイトル: 「APIサーバー構築フロー（サンプル）」
- Mermaid図: `graph TD` での設計〜実装〜テスト〜デプロイのフロー
- ステップ詳細: 各フェーズの補足説明をグリッドカードで配置
- スタイル: ダークテーマ、アクセント `#00ffcc`、シャープな角

## 8. エラー処理

- `user-context` プロファイルが未ロードの場合: このスキルの実行を中止し、`user-context` の完了を要求する。
- ブラウザ起動コマンドが失敗した場合: ファイルパスをユーザーに提示し、手動で開くよう案内する。
- `/tmp/agents_plans/` ディレクトリの作成に失敗した場合: エラーを報告し、代替の保存先をユーザーに確認する。
