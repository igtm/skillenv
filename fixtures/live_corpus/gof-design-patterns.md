---
name: gof-design-patterns
description: >-
  GoF（Gang of
  Four）デザインパターン全23種のリファレンス。生成・構造・振舞の3カテゴリに分類され、各パターンの概要、適用場面、Don't/Doの具体的事例、長所・短所、他パターンとの関係を提供します。コードレビューや設計相談に活用してください。
---
# GoF Design Patterns Skill

このスキルは、GoF（Gang of Four）の23種類のデザインパターンについてのリファレンスを提供します。各パターンは **Don't（やってはいけない）** と **Do（こうすべき）** の事例付きでまとめられています。

## 概要

refactoring.guru の日本語ドキュメントに基づき、各パターンの概要、問題、解決策、適用場面、長所・短所、他パターンとの関係を構造化しています。

### ドキュメント構成

全23パターンが3カテゴリ × 個別ファイルで整理されています。

#### 生成関連パターン（Creational Patterns）
オブジェクト生成の柔軟性とコード再利用を促進するパターン群。

- `docs/creational/factory_method.md` - Factory Method: サブクラスで生成するオブジェクトの型を変更可能にする
- `docs/creational/abstract_factory.md` - Abstract Factory: 関連オブジェクト群を具象クラスに依存せず生成する
- `docs/creational/builder.md` - Builder: 複雑なオブジェクトを段階的に構築する
- `docs/creational/prototype.md` - Prototype: 既存オブジェクトのクローンを生成する
- `docs/creational/singleton.md` - Singleton: クラスのインスタンスが一つだけであることを保証する

#### 構造関連パターン（Structural Patterns）
オブジェクトやクラスを大きな構造に柔軟・効率的に束ねるパターン群。

- `docs/structural/adapter.md` - Adapter: 互換性のないインターフェース間を橋渡しする
- `docs/structural/bridge.md` - Bridge: 抽象化と実装を分離し、独立して変更可能にする
- `docs/structural/composite.md` - Composite: オブジェクトをツリー構造に組み立て、個々と全体を同一に扱う
- `docs/structural/decorator.md` - Decorator: オブジェクトにラッパーで動的に新しい振る舞いを追加する
- `docs/structural/facade.md` - Facade: 複雑なサブシステムにシンプルなインターフェースを提供する
- `docs/structural/flyweight.md` - Flyweight: 共有で大量のオブジェクトのメモリ使用を最適化する
- `docs/structural/proxy.md` - Proxy: 別オブジェクトの代理として振る舞い、アクセスを制御する

#### 振舞関連パターン（Behavioral Patterns）
アルゴリズムやオブジェクト間の責任分担に関するパターン群。

- `docs/behavioral/chain_of_responsibility.md` - Chain of Responsibility: リクエストをハンドラーの連鎖に沿って渡す
- `docs/behavioral/command.md` - Command: リクエストをオブジェクトとしてカプセル化する
- `docs/behavioral/iterator.md` - Iterator: コレクションの要素を順次アクセスする
- `docs/behavioral/mediator.md` - Mediator: オブジェクト間の直接通信を制限し、仲介者を介させる
- `docs/behavioral/memento.md` - Memento: オブジェクトの状態のスナップショットを保存・復元する
- `docs/behavioral/observer.md` - Observer: オブジェクトの状態変化を複数の依存オブジェクトに通知する
- `docs/behavioral/state.md` - State: 内部状態の変化に応じてオブジェクトの振る舞いを変更する
- `docs/behavioral/strategy.md` - Strategy: アルゴリズムのファミリーを定義し、交換可能にする
- `docs/behavioral/template_method.md` - Template Method: アルゴリズムの骨格を定義し、ステップをサブクラスに委譲する
- `docs/behavioral/visitor.md` - Visitor: 既存クラスを変更せずに新しい操作を追加する

## このスキルの使い方

中大規模コードの実装やリファクタリング時に、設計上の課題に対してどのパターンを適用すればよりメンテナブルな構造になるかを判断するためのリファレンスとして使用してください。

ユーザーのコードに以下のような兆候（コードの匂い）が見られた場合、対応するパターンの `docs/` ファイルを参照し、Don't/Do の事例を基に具体的な改善方針を提示してください。

### コードの匂い → 検討すべきパターン

| コードの匂い・設計課題 | 検討パターン | 参照ファイル |
|---|---|---|
| `new` の直接呼び出しが散在し、具象クラスに密結合 | Factory Method → Abstract Factory | `docs/creational/factory_method.md`, `abstract_factory.md` |
| コンストラクターのパラメータが多すぎる（望遠鏡的コンストラクター） | Builder | `docs/creational/builder.md` |
| 継承で複数の次元を組み合わせ、サブクラスが爆発的に増加 | Bridge, Strategy, Decorator | `docs/structural/bridge.md`, `docs/behavioral/strategy.md`, `docs/structural/decorator.md` |
| `if/switch` による型・状態の条件分岐が多数のメソッドに分散 | State, Strategy | `docs/behavioral/state.md`, `docs/behavioral/strategy.md` |
| 既存クラスを変更せずに振る舞いや操作を追加したい | Decorator, Visitor, Observer | `docs/structural/decorator.md`, `docs/behavioral/visitor.md`, `docs/behavioral/observer.md` |
| 外部ライブラリや既存コードのインターフェースが合わない | Adapter | `docs/structural/adapter.md` |
| 複雑なサブシステムの初期化手順がクライアントに漏洩 | Facade | `docs/structural/facade.md` |
| 大量の類似オブジェクトでメモリ消費が問題 | Flyweight | `docs/structural/flyweight.md` |
| 重いリソースの遅延初期化・アクセス制御・キャッシュが必要 | Proxy | `docs/structural/proxy.md` |
| ツリー構造で個別要素と集合を統一的に扱いたい | Composite | `docs/structural/composite.md` |
| リクエストの処理順序を柔軟に構成・変更したい | Chain of Responsibility | `docs/behavioral/chain_of_responsibility.md` |
| 操作の取り消し（Undo）・キュー・遅延実行が必要 | Command + Memento | `docs/behavioral/command.md`, `docs/behavioral/memento.md` |
| コレクションの内部構造を隠蔽しつつ走査したい | Iterator | `docs/behavioral/iterator.md` |
| コンポーネント間の双方向依存が複雑に絡み合っている | Mediator | `docs/behavioral/mediator.md` |
| あるオブジェクトの状態変化を複数箇所に通知したい | Observer | `docs/behavioral/observer.md` |
| アルゴリズムの骨格は共通だがステップの詳細が異なるクラスが複数ある | Template Method | `docs/behavioral/template_method.md` |
| グローバルなシングルインスタンスが必要（ただし慎重に） | Singleton（※DI での代替も検討） | `docs/creational/singleton.md` |
| 既存オブジェクトの完全なコピーが必要で具象クラスに依存したくない | Prototype | `docs/creational/prototype.md` |

### 設計判断の指針

- **まず委譲（コンポジション）を検討**し、継承は最後の手段とする。Bridge, Strategy, Decorator, State は全て委譲ベースのパターン。
- **開放閉鎖の原則（OCP）を意識**: 既存コードの修正なく拡張できる設計を目指す。Factory Method, Decorator, Observer, Strategy 等が有効。
- **過度な適用を避ける**: パターンは「問題がある場合」に適用するもの。問題のないコードにパターンを強制するとかえって複雑化する。
- **パターンの発展経路を把握**: Factory Method → Abstract Factory → Builder のように、シンプルなパターンから始めて必要に応じて複雑なパターンへ移行する。

## 関連リンク

- 出典: https://refactoring.guru/ja/design-patterns/catalog
