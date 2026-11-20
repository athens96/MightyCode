---
title: 画面構成
order: 2
section: layout
---
ウインドウは左のサイドバーと右の作業エリアに分かれています。作業エリアでは、ペインをタブとして重ねたり、左右・上下に分割して並べたりできます。

![[overview]]

## サイドバー {#sidebar}
サイドバーには上から順に次のものがあります。
- {{ui:sidebar.searchPlaceholder}}: {{kbd:⌘K}}を押すと検索欄に直接移動します。
- {{ui:phone.dashboard.title}}: すべてのワークスペースのペインの状態を1か所で確認します。
- {{ui:review.sidebar}}: レビューする変更があるペインとリクエストをまとめて確認します([レビュー待ち](#review))。
- {{ui:sidebar.workspacesHeader}}の一覧: ワークスペースをクリックすると、そのワークスペースに切り替わります。横の矢印をクリックすると、ペインの一覧を展開したり折りたたんだりします。
- 下部: ヘルプボタン、画面テーマのボタン(ダーク/ライト)と設定ボタン。

ワークスペースの一覧を展開すると、最後の行に{{ui:workspace.addPane}}ボタンがあります。ペインの行を右クリックすると、{{ui:menu.rename}}、{{ui:menu.closePane}}の項目があります。

### サイドバーの幅を調整する {#sidebar-resize}
サイドバーと作業エリアの境界線をドラッグすると、サイドバーの幅が変わります({{ui:sidebar.resize}})。
- 境界線にポインタを合わせると、アクセントカラーに変わります。左右にドラッグして幅を合わせます。幅は決まった範囲の中でだけ変わり、アプリを開き直しても保たれます。
- 境界線をダブルクリックすると、既定の幅に戻ります。
- 境界線をTabキーで選んで{{kbd:←}} / {{kbd:→}}を押すと、10pxずつ狭く・広くなります。
- 境界線を左端までドラッグすると、サイドバーが折りたたまれます。再び展開すると、ドラッグする前の幅に戻ります。

### サイドバーを折りたたむ {#sidebar-collapse}
作業エリアを広く使いたいときは、サイドバーを折りたたみます。
1. 作業エリアのヘッダー左上にあるサイドバーボタンをクリックするか、{{kbd:⌃⌘S}}を押します。{{ui:phone.dashboard.title}}の画面と最初の画面にも、同じ位置にこのボタンがあります。メニューバーの「表示」メニューにも{{ui:sidebar.collapse}}の項目があります。
2. 再び展開するときも、同じボタンか{{kbd:⌃⌘S}}を使います({{ui:sidebar.expand}})。

折りたたんだ状態は、アプリを開き直しても保たれます。折りたたんだ状態で{{kbd:⌘K}}を押すと、サイドバーが展開されて検索欄に移動します。

![[sidebar-collapsed]]

## 作業状況 {#dashboard}
サイドバーの検索欄の下にある{{ui:phone.dashboard.title}}をクリックすると、すべてのワークスペースのペインを状態順にまとめて表示します。
- 上部の欄で、{{ui:phone.dashboard.stat.running}}、{{ui:phone.dashboard.stat.waiting}}、{{ui:phone.dashboard.stat.done}}の件数を確認します。
- {{ui:dashboard.usage.title}}カードでアカウントの使用量を確認します。同じアカウントを使うほかのアプリやセッションと共有する上限です。
- ワークスペースごとに{{ui:files.pane.title}}ボタンと{{ui:workspace.addPane}}メニューがあります。
- カードをクリックすると、そのペインに移動します。ワークスペースかペインを選ぶと、作業状況から抜けます。

![[dashboard]]

## レビュー待ち {#review}
レビューするものを一か所にまとめて見ます。サイドバーの{{ui:phone.dashboard.title}}の下にある{{ui:review.sidebar}}を押すか、{{kbd:⇧⌘R}}(Windows: {{kbd:Ctrl+Shift+R}})を押すと{{ui:review.open}}パネルが開きます。項目の横の数字は待っている件数です。
- 表示されるもの: 変更を残して終わった[worktreeペイン](delegation.md#pane-worktree)と[子ペイン](delegation.md#states)、そしてペインごとに最後に終わったリクエストのうち変更したファイルが保管されたもの([リクエストの変更をレビュー](agent-pane.md#request-changes))です。まだ作業中のペインは終わるまで表示されません。
- 各行にペインの名前、エージェント、種類、ワークスペース、ブランチとベースブランチ、変更したファイル数と追加・削除した行数、状態({{ui:review.status.ready}}、{{ui:review.status.conflict}}、{{ui:review.status.merged}})が表示されます。競合が一番上、マージ済みが一番下です。
- ボタン: {{ui:review.action.changes}}、{{ui:review.action.merge}}(コミットしていないファイルがあるworktreeペインは{{ui:review.action.commitMerge}})、{{ui:editor.open}}、{{ui:review.action.pr}}(worktreeペイン)、{{ui:review.action.discard}}、{{ui:review.action.focus}}、{{ui:review.action.dismiss}}。マージ・破棄・PRはペインのバーや子のカードと同じ操作なので、同じ確認を経ます。リクエストの{{ui:review.action.changes}}はリクエストの変更レビューを開きます。
- {{ui:review.action.dismiss}}を押すと、その項目は作業が再び変わるまで隠れます。
- 一覧は実行が終わったとき、パネルを開いたとき、{{ui:review.refresh}}を押したときにgitを読み直します。描画のたびには読みません。

## ペインを追加する {#add-pane}
ペインは、AIエージェント(Claude、Codex、Antigravity、pi、opencode、Hermes)、ターミナル、ファイルのいずれかです。

1. サイドバーの{{ui:workspace.addPane}}ボタンをクリックします。
2. 作成するペインを選びます: エージェント(Claude、Codex、Antigravity、pi、opencode、Hermes)のペイン、{{ui:workspace.newTerminal}}、または{{ui:menu.showFiles}}({{kbd:⇧⌘E}})。メニューの一番下のまとまりには、ワークスペースを追加する3つの方法、{{ui:workspace.add.existing}}、{{ui:workspace.add.newFolder}}、{{ui:workspace.add.fromGit}}があります([ワークスペースを開く](getting-started.md#open-workspace))。
3. 新しいAIペインは、同じエージェントで最後に使ったペインのモデル、思考レベル、権限、表示方法を引き継ぎます。会話は新しく始まります。

ショートカット: {{ui:menu.newClaudePane}}は{{kbd:⌘N}}、{{ui:menu.addTerminalPane}}は{{kbd:⌘T}}、{{ui:menu.showFiles}}は{{kbd:⇧⌘E}}です。実行ペインは、すべてのワークスペースを合わせて128個まで開けます(ほかのコンピュータのペインをそのまま表示するリモートペインは数えません)。

> [!note]
> Claude以外のエージェント(Codex、Antigravity、pi、opencode、Hermes)のペインには{{ui:badge.beta}}の表示が付きます。

### 以前のセッションを続ける {#resume}
エージェントのペインを追加するとき、このフォルダに以前のセッションがあると選択画面が表示されます。Antigravityはこのフォルダの会話が表示され、サブエージェントが実行した会話は表示されません。
1. 新しい会話なら{{ui:resume.choice.startNew}}、以前の会話なら{{ui:resume.choice.resume}}ボタンをクリックします。
2. {{ui:resume.title}}の一覧からセッションを選びます。{{ui:resume.search}}で探すこともできます。
3. 新しいペインがそのセッションの続きから始まります。

![[resume-choice]]

![[resume-list]]

### 過去の会話を検索 {#conversation-search}
{{ui:conversationSearch.open}}（{{kbd:⇧⌘F}}）は、このフォルダの過去のセッションのリクエストと回答から言葉を探します。ワークスペースのヘッダーの虫めがねボタン、サイドバーでワークスペースを右クリックしたメニュー、メニューバーのワークスペースメニューからも開けます。
- 一つのメッセージにすべての単語が含まれるセッションが新しい順に最大50件表示され、見つかった言葉に印が付きます。ツールの出力は検索しません。
- Claude、Codex、Antigravity、piのセッションを検索し、opencodeとHermesのセッションは検索しません。ペインで開いているセッションと60日より古いセッションは含まれません。
- {{kbd:↑}} {{kbd:↓}}で選んで{{kbd:Enter}}を押すか、{{ui:conversationSearch.resume}}をクリックすると、新しいペインがそのセッションを続けます。
- {{ui:conversationSearch.export}}は、その会話のリクエストと回答をMarkdownファイルに保存します。
- リモートワークスペースでは使えません。セッションの記録がそのコンピュータにあるためです。

## タブと分割 {#tabs-splits}
- タブをドラッグしてほかのグループの中央にドロップすると、タブとしてまとまります({{ui:layout.drop.merge}})。
- グループの左・右・上・下の端にドロップすると、その方向に分割されます。
- 分割の境界線をドラッグするとサイズが変わり、ダブルクリックすると半分ずつに分かれます。
- ドラッグ中に{{kbd:Esc}}を押すとキャンセルされます。
- タブをダブルクリックすると名前を変更します。右クリックすると、{{ui:menu.rename}}、{{ui:menu.focusPane}}、{{ui:menu.closeTab}}の項目があります。

配置はワークスペースごとに保存されます。ワークスペースを切り替えても、それぞれの配置がそのまま残ります。

### ペインのメニュー {#pane-menu}
ペインのヘッダーにある…ボタンには、次の項目があります。
- {{ui:menu.rename}}
- {{ui:menu.focusPane}}: このペインだけを大きく表示します。もう一度選ぶと{{ui:pane.menu.restoreLayout}}で元に戻ります。
- {{ui:pane.menu.copyLog}}: エージェントペインの実行記録をコピーします。ターミナルペインでは、この位置に{{ui:pane.menu.terminalHistory}}があります([ターミナルペイン](terminal-files-browser.md#terminal))。
- {{ui:pane.menu.newConversation}}: 次のリクエストからは以前の会話を引き継ぎません(エージェントペイン)。
- {{ui:menu.closePane}}

ペインのヘッダーを右クリックすると{{ui:menu.rename}}、{{ui:session.menu.focus}}、{{ui:session.menu.close}}の項目があり、ターミナルペインには{{ui:terminal.history.title}}もあります。

## ショートカット {#shortcuts}
| ショートカット | 動作 |
|---|---|
| {{kbd:⌘O}} | {{ui:menu.openProject}} |
| {{kbd:⌘N}} | {{ui:menu.newClaudePane}} |
| {{kbd:⌘T}} | {{ui:menu.addTerminalPane}} |
| {{kbd:⇧⌘E}} | {{ui:menu.showFiles}} |
| {{kbd:⇧⌘F}} | {{ui:conversationSearch.open}} |
| {{kbd:⇧⌘R}} | {{ui:review.open}} |
| {{kbd:⌘K}} | {{ui:menu.searchWorkspaces}} |
| {{kbd:⌃⌘S}} | {{ui:sidebar.collapse}} / {{ui:sidebar.expand}} |
| {{kbd:⌘,}} | {{ui:menu.settings}} |
| {{kbd:Enter}} | リクエストを送信。実行中のペインでは{{ui:composer.busySend.title}}に従ってキューに入れるか、選んだエージェントの新しいworktreeですぐ実行([実行中に続けてリクエストする](agent-pane.md#queue)) |
| {{kbd:⇧Enter}} | 改行 |
| {{kbd:⌘Enter}} | 実行中のペインで、新しいworktreeですぐ実行([Mightyオーケストレーション](delegation.md#worktree)がオンで、gitリポジトリにあるペイン)。それ以外の実行中のペインではキューに追加 |
| {{kbd:Esc}} | シートを閉じる、スラッシュコマンドの一覧を閉じる |
| {{kbd:⌘=}} / {{kbd:⌘-}} / {{kbd:⌘0}} | {{ui:menu.textBigger}} / {{ui:menu.textSmaller}} / {{ui:menu.textActualSize}} |
| {{kbd:⌘?}} | {{ui:menu.help}} |
