---
title: エージェントペイン
order: 3
section: agent-pane
---
エージェントペインは、Claude、Codex、Antigravity、pi、opencode、Hermesのいずれかと会話するペインです。下部の入力欄にリクエストを書き、上部で進行状況と結果を確認します。

![[agent-basic]]

## リクエストを送信する {#send}
1. 入力欄にやってほしいことを書きます。
2. {{kbd:Enter}}を押すか、送信ボタンをクリックします。改行するには{{kbd:⇧Enter}}を押します。
3. 実行中は送信ボタンの位置に停止ボタンが表示されます。止めるにはそのボタンをクリックします。

日本語入力のように変換中の文字があるときに押した{{kbd:Enter}}は、送信せずに変換の確定だけを行います。

![[composer]]

### 実行中に続けてリクエストする {#queue}
タスクの実行中でも、次のリクエストを書けます。入力欄に文字があると、停止ボタンの横に送信ボタンと同じ丸いボタンが表示されます。
- {{kbd:Enter}}または{{ui:queue.add}}ボタン: 次のリクエストとしてキューに入れます。現在のタスクが終わると順番に実行します。1つのペインに16件まで入れられます。下の{{ui:composer.busySend.title}}でエージェントを選んであれば、代わりにそのエージェントで新しいworktreeですぐ実行します。
- {{ui:composer.busySend.title}}: 実行中だけ表示される選択で、文字を入力すると送信ボタンとひとつにつながり[対象 ▾ | ↑]の形になります(入力がないときは小さなカプセルとして単独で表示されます)。対象が{{ui:composer.busySend.queue}}のときだけ、その左に{{ui:queue.worktree}}ボタンが別に表示され、worktreeを選んでいると送信ボタンが同じことをするので隠れます。{{ui:composer.busySend.queue}}ならリストのアイコン、エージェントを選んでいればそのエージェントのアイコンが表示され、ポインタを合わせると今の選択が表示されます。{{ui:composer.busySend.queue}}(既定)と、このペインが子として始められるエージェントごとの「worktree · Codex」のような項目があります(Claude、Codex、Antigravity)。CLIがインストール・サインインされていない、合うモードがない、または自分で開いた子ペインがすでに3つある項目はグレーになり、理由が表示されます。選んだ値によって、{{kbd:Enter}}と丸いボタンはリクエストをキューに入れるか、このペインの隣にそのエージェントの子ペインを開いて新しいworktreeですぐ実行します。入力欄の案内とボタンの名前もそれに合わせて変わります。選んだ値はアプリを終了するまでペインごとに覚えています。テキストがなく添付ファイルだけのリクエストはキューに入ります。
- {{kbd:⌘Enter}}(Windowsでは{{kbd:Ctrl+Enter}})または{{ui:queue.worktree}}ボタン: {{ui:composer.busySend.title}}に関係なく、いつもこのペインのエージェントを使います。MightyオーケストレーションがオンのClaudeペイン、または子がファイルを変更できるモードのCodex・Antigravityペインが、コミットのあるブランチのgitリポジトリにあるときは、このペインの隣に専用のgit worktreeを持つ新しい子ペインを開いてすぐに実行します。実行中のタスクはそのまま続き、子が終わったらサイドバーのカードで結果をマージします([新しいworktreeで実行する](delegation.md#worktree))。
- 子ペイン、そのようなリポジトリの外のペイン、Mightyオーケストレーションをオフにしたペインでは、{{kbd:⌘Enter}}も{{kbd:Enter}}と同じようにキューに入れます。このペインのエージェントが今のモードでは子を作れなくても別のエージェントなら作れるときは、{{kbd:⌘Enter}}はキューに入れますが、{{ui:composer.busySend.title}}と下の{{ui:queue.runNow}}のエージェント選択で、そのエージェントの子ペインで実行できます。子を作れないときや始められないときも、テキストはキューに入ります([新しいworktreeで実行する](delegation.md#worktree))。送ったテキストが実行中のタスクに割り込むことはもうありません。以前の記録のMighty表示には、そうしたテキストがオレンジ色の{{ui:graph.block.steer}}ブロックとして残っています。実行中のClaudeの親にそのまま届いた委任の通知は、同じ色の{{ui:graph.block.delegationNotice}}ブロックとして表示されます([通知と保留](delegation.md#notices))。
- キューの各リクエストには、✕の隣に{{ui:queue.runNow}}ボタンがあります。新しいworktreeを作れる場所({{kbd:⌘Enter}}の条件か、別のエージェントなら子を作れるペイン)では、そのリクエストを新しい子ペインですぐに実行し、実行中のタスクはそのまま続きます。ボタンの前のエージェント選択(既定はこのペインのエージェント)で別のエージェントを選ぶと、そのエージェントの子ペインで実行します。ボタンのツールチップが、どちらになるかを教えてくれます。テキストがなく添付ファイルだけのリクエストは、worktreeには行きません。
- worktreeを作れない場所では、このペインで処理します。エラーで止まったペインのように待機中なら、そのリクエストをすぐに始めます。Claudeの応答が終わってバックグラウンドタスクだけを待っているペインでは、リクエストが実行中のClaudeにそのまま届き、Mighty表示に{{ui:graph.block.steer}}ブロックとして表示されます(添付のあるリクエストはタスクが終わるまで待ちます)。それ以外ではボタンがグレーになり、ツールチップが理由を教えてくれます。今のタスクが先に終わる必要がある、このペインに保留された項目が先に送られる、CLIの更新中、のいずれかです。そのリクエストは順番を待ちます。Antigravityのカードの続きのリクエストは、いつも自分のペインでだけ実行されます。
- キューのリクエストは鉛筆ボタン（{{ui:queue.edit}}）でその場で文を編集できます。{{kbd:Enter}}で保存、{{kbd:Esc}}で取り消します（{{kbd:Shift+Enter}}は改行）。添付ファイルとモードはそのままで、文を空にすると保存されません。許可したカードの続きのリクエストは編集できません。
- ↑・↓ボタン（{{ui:queue.moveUp}}・{{ui:queue.moveDown}}）、または行を選んだまま{{kbd:↑}}・{{kbd:↓}}キーで順番を変えます。一番上のリクエストが次に実行され、キューは新しい順番で実行します。保留された行は常に一番上にあり、編集も移動もできません。

タスクを停止すると、待機中のリクエストもキャンセルされます。ただし、バックグラウンドの作業だけを待っているペインで停止すると、キューはそのまま残り、最初のリクエストがすぐに始まります。アプリを開き直すとキューは空になります。

## 入力欄のツール {#composer}
入力欄の下の行には、左から添付、モデル、思考レベル、権限、その他の実行設定(…)が並んでいます。ペインの幅が狭いと、一部が1つのメニューにまとめられます。実行中は変更できず、変更した値は次のリクエストから適用されます。

{{ui:compare.menu}}: 待機中のペインでは送信ボタンの右クリックメニュー、実行中のペインでは {{ui:composer.busySend.title}} メニューにあります。入力欄の依頼を同じ基準コミットから複数のエージェントに同時に任せ、結果を並べて比べます([比較実行](delegation.md#compare))。

### モデル {#model}
モデルのチップをクリックすると、{{ui:composer.label.runner}}(Claude、Codex、Antigravity、pi、opencode、Hermes)と{{ui:composer.label.model}}を選べます。一覧が正しくない場合は{{ui:composer.model.refresh}}をクリックしてください。エージェントを変更すると、会話は新しく始まります。

### 思考レベル {#effort}
{{ui:composer.effort.label}}は、モデルがどれだけ深く考えるかを決めます。Auto、Low、Medium、High、XHigh、Maxから選びます。モデルが対応している段階だけが表示されます。Codexのモデルが対応していれば{{ui:composer.effort.ultra}}もあります。最も深く考え、作業の一部をCodex自身のサブエージェントに任せることがあるため、使用量が増えることがあります。Antigravityにはなく、piはOff、Minimal、Low、Medium、High、XHighで、opencodeはモデルが提供するバリアントのLow、Medium、High、Maxで、HermesはLow、Medium、High、XHigh、Maxです。

### 作業の権限 {#permission}
権限は、エージェントが確認なしで行える作業の範囲です。

| エージェント | 選べる権限 |
|---|---|
| Claude | {{ui:permission.label.claude.plan}} · {{ui:permission.label.claude.default}} · {{ui:permission.label.claude.acceptEdits}} · {{ui:permission.label.claude.auto}} · {{ui:permission.label.claude.fullAccess}} |
| Codex | {{ui:permission.label.defaultCodex}} · {{ui:permission.label.onRequest}} · {{ui:permission.label.acceptEditsCodex}} · {{ui:permission.label.fullAccess}} |
| Antigravity | {{ui:permission.label.default}} · {{ui:permission.label.plan}} · {{ui:permission.label.acceptEdits}} · {{ui:permission.label.fullAccess}} |
| pi | {{ui:permission.label.defaultCodex}} · {{ui:permission.label.fullAccess}} |
| opencode | {{ui:permission.label.default}} · {{ui:permission.label.plan}} · {{ui:permission.label.acceptEdits}} · {{ui:permission.label.fullAccess}} |
| Hermes | {{ui:permission.label.default}} · {{ui:permission.label.acceptEdits}} · {{ui:permission.label.fullAccess}} |

- Claudeの{{ui:permission.label.claude.default}}、Codexの{{ui:permission.label.onRequest}}、{{ui:permission.label.fullAccess}}以外のopencodeのすべてのモード、Hermesのファイル変更と危険なコマンドでは、追加の権限が必要になったときにペイン内で許可を求めます。[許可と質問](approvals.md#permission)を参照してください。
- {{ui:permission.label.claude.auto}}は、インストールされているClaude CLIが対応している場合にのみ表示されます。Codexの{{ui:permission.label.onRequest}}は、対応していない古いCodexでは表示されません。
- {{ui:permission.label.claude.fullAccess}}と{{ui:permission.label.fullAccess}}は、プロジェクト外のファイルやコマンドにもアクセスします。本当に必要なときだけ使ってください。

### Antigravity {#antigravity}
GoogleのエージェントはAntigravity CLI(`agy`)で動きます。AntigravityはClaudeやGPT-OSSのモデルも実行できます。

- インストール: Macでは`curl -fsSL https://antigravity.google/cli/install.sh | bash`を実行すると、`~/.local/bin`に`agy`が入ります。WindowsではPowerShellで`irm https://antigravity.google/cli/install.ps1 | iex`を実行すると、`%LOCALAPPDATA%\agy\bin`に`agy.exe`が入ります。アップデートはアプリが`agy update`で行います。
- サインイン: ターミナルペインで`agy`を一度実行し、ブラウザでGoogleアカウントにサインインします。{{ui:settings.nav.tools}}タブの{{ui:settings.cliAccounts.buttonLogin}}をクリックすると、アプリが代わりに実行します。Antigravityにはサインアウトのコマンドがないため、アカウントを切り替えるには`agy`の中で`/logout`を使います。
- 権限: Antigravityはアプリの中で確認できません。{{ui:permission.label.default}}はagyに保存されたAgent Modeに従います(agyの`/config`で変えていなければレビュー)。ワークスペースのファイルは読めますが、確認が必要なファイル編集とシェルコマンドは拒否し、ターンを終えてから、拒否した操作を権限カードで尋ねます。{{ui:permission.allowOnce}}または{{ui:antigravity.permission.allowAlways}}を押すと、同じ会話で続きが進みます([Antigravityの権限カード](approvals.md#antigravity-permission))。{{ui:permission.label.plan}}はagy自身のフォルダに計画を書き、プロジェクトを変更せずに止まります。実行するには、次のリクエストを別のモードで送ってください。あらかじめ許可しておきたい操作は、`~/.gemini/antigravity-cli/settings.json`の`permissions.allow`に許可ルール(例: `command(git status)`。ルールはそのテキストで始まるすべてのコマンドを許可するので、できるだけ狭く)を自分で追加するか、{{ui:permission.label.acceptEdits}}(ファイル編集)または{{ui:permission.label.fullAccess}}(すべて許可)を選んでください。
- Antigravityのモデルは名前に思考の深さが含まれているため、{{ui:composer.effort.label}}を別に選びません。
- {{ui:graph.view.mighty}}表示はClaudeやCodexと同じです。Antigravityが起動したサブエージェントはリクエストの下にそれぞれのブロックになり、その内容(受けた指示、メッセージ、ツール呼び出し、送り返した答え)は`~/.gemini/antigravity-cli`(Windowsでは`%USERPROFILE%\.gemini\antigravity-cli`)に残るサブエージェント自身の記録から読みます。読んだファイルは名前だけが表示され、中身は写しません。会話の以前のリクエストも同じ記録から読み込み、{{ui:resume.choice.resume}}にはこのフォルダのAntigravityの会話が表示されます。
- 以前Gemini CLIで実行していたペインは、履歴はそのままですが次のリクエストから新しい会話になり、Gemini CLIのモデルを選んでいた場合は既定に戻ります。

### pi {#pi}
pi(`pi`)は、多くのモデル提供元で使えるオープンソースのコーディングエージェントCLIです。

- インストール: {{ui:setup.title}}画面からインストールします(HomebrewとNode.jsがなければ先にインストールします)。自分でインストールするなら`npm install -g @mariozechner/pi-coding-agent`を実行します。アップデートはアプリが`pi update self`で行います。
- サインイン: {{ui:settings.nav.tools}}タブの{{ui:settings.cliAccounts.buttonLogin}}を押すと、`pi`を実行するターミナルペインが開きます。そこで`/login`と入力し、サブスクリプション(Claude Pro/Max、ChatGPT、GitHub Copilot)またはAPIキーを選びます。環境変数のAPIキー(`ANTHROPIC_API_KEY`など)や`~/.pi/agent/auth.json`に保存された認証情報もサインイン済みと見なします。サインアウトはpiの中で`/logout`と入力します。
- モデル: サインインすると、`pi --list-models`の一覧が「提供元/モデル」の名前で表示され、先頭の項目はpi自身の設定に従います。思考機能のないモデルでは{{ui:composer.effort.label}}は表示されません。段階はOff、Minimal、Low、Medium、High、XHighです。
- 権限: piには承認ウィンドウがないため、モードは2つです。{{ui:permission.label.defaultCodex}}モードは読み取りツール(read、grep、find、ls)だけを有効にします。{{ui:permission.label.fullAccess}}モードはすべてのツールを確認なしで使い、プロジェクトの外にも触れられます。
- リクエストごとにペインの作業フォルダで`pi --mode rpc`を実行し、ペインのセッションを引き継ぎます。セッションは`~/.pi/agent/sessions`に保存され、{{ui:resume.choice.resume}}にもこのフォルダのpiセッションが表示されます。
- 質問: piには質問ツールがないため、アプリが実行のたびに`ask_user`ツールを拡張機能として追加します（`~/.pi`の設定は変更しません）。piがこのツールで質問すると[質問カード](approvals.md#questions)が表示され、答えると同じリクエストが続きます。ほかの拡張機能が開く選択・確認・入力のダイアログも同じカードで表示されます。
- まだできないこと: piのペインは[Mighty Orchestration](delegation.md)に参加せず、[リモートワークスペース](remote.md)では開けず、承認カードもありません。

### opencode {#opencode}
opencode(`opencode`)は、多くのモデル提供元で使えるオープンソースのコーディングエージェントCLIです。自社サービスopencode Zenの無料モデルはサインインなしでも使えます。

- インストール: {{ui:setup.title}}画面からインストールします(HomebrewとNode.jsがなければ先にインストールします)。自分でインストールするなら`npm install -g @opencode/cli`を実行します。アプリはOpenCode 2を使います。MacにOpenCode 1があると古いものとして表示され、セットアップ画面からインストールするとnpmの`opencode-ai`を先に削除します。アップデートはアプリが`opencode upgrade`で行います。
- サインイン: {{ui:settings.nav.tools}}タブの{{ui:settings.cliAccounts.buttonLogin}}を押すと、`opencode auth login`を実行するターミナルペインが開きます。そこで提供元を選び、そのサブスクリプションでサインインするかAPIキーを入力します。opencodeが保存したサインイン(`opencode auth list`)や環境変数のAPIキー(`ANTHROPIC_API_KEY`など)もサインイン済みと見なし、どちらもなければopencode Zenの無料モデルだけが表示されます。ログアウトはターミナルで`opencode auth logout`を実行します。
- モデル: `opencode models`の一覧が「提供元/モデル」の名前で表示され、先頭の項目はopencode自身の設定に従います。
- 権限: 選んだモードがそのリクエストだけのopencode自身のルールになり、ルールが確認に残した操作はClaudeと同じくペインに承認カードとして届きます。{{ui:permission.label.default}}はファイルの編集、コマンド、Web、プロジェクト外へのアクセスの前に確認し、{{ui:permission.label.acceptEdits}}はファイルの編集を確認せず、{{ui:permission.label.plan}}はファイルを編集しないopencodeのplanエージェントで実行し、{{ui:permission.label.fullAccess}}は確認しません。
- リクエストごとにペインの作業フォルダで`opencode acp`(OpenCode 2、Agent Client Protocol)を実行し、ペインのセッションを引き継ぎます。セッションはopencodeの`opencode.db`に保存され、{{ui:resume.choice.resume}}にもこのフォルダのopencodeセッションが表示されます。使用量にはセッションの費用、使用中のコンテキスト、リクエストのトークンが表示されます。
- 質問: opencodeが`question`ツールで質問すると[質問カード](approvals.md#questions)が表示され、答えると同じリクエストが続きます。カードで{{ui:phone.questionnaire.cancel}}をクリックすると、opencodeはそのリクエストを終了します。
- まだできないこと: opencodeのペインは[Mighty Orchestration](delegation.md)に参加せず、[リモートワークスペース](remote.md)では開けません。

### Hermes {#hermes}
Hermes(`hermes`)は、Nous ResearchのオープンソースのエージェントCLI(Hermes Agent)です。Nous Portal、OpenRouter、Anthropic、OpenAIなど多くのモデル提供元を使えます。

- インストール: {{ui:setup.title}}画面からインストールします(Gitがなければ先にインストールします)。公式のインストールスクリプトが専用のPython環境と一緒に`~/.hermes`へインストールし、ブラウザツールもダウンロードするため数分かかります。自分でインストールする場合は、Macでは`curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash`を、WindowsではPowerShellで`irm https://hermes-agent.nousresearch.com/install.ps1 | iex`を実行します。アップデートはアプリが`hermes update`で行います。
- サインイン: {{ui:settings.nav.tools}}タブの{{ui:settings.cliAccounts.buttonLogin}}ボタンをクリックすると、`hermes model`を実行するターミナルペインが開きます。そこでモデルの提供元を選び、サインインするかAPIキーを入力してから、既定のモデルを選びます。アプリが読むのは、Hermesが資格情報を見つけた提供元の名前だけです。資格情報を消すにはターミナルで`hermes auth`を使います。
- モデル: 最初の項目はHermes自体の設定(`hermes model`で決めた既定のモデル)に従います。Hermesはセッションを開いて初めてモデル一覧を示すため、最初のリクエストを送ったあとから`提供元:モデル`の一覧が表示されます。
- 権限: {{ui:permission.label.default}}はファイルを変更するたびにペインの承認カードで確認し、{{ui:permission.label.acceptEdits}}は作業フォルダ内の変更を確認せず、{{ui:permission.label.fullAccess}}はHermesの`--yolo`で危険なコマンドも確認しません。危険なコマンド(`rm -rf`など)はHermesの承認設定(`approvals.mode`)に従ってカードで確認し、それ以外のコマンドと読み取りは確認しません。計画専用のモードはありません。{{ui:permission.label.fullAccess}}はセッション開始時に決まるため、すでに始めたセッションには新しい会話から適用されます。
- リクエストごとにペインの作業フォルダで`hermes acp`(Agent Client Protocol)を実行し、ペインのセッションを引き継ぎます。実際の作業はHermesのゲートウェイ(バックグラウンドプロセス)が行い、必要なときに自動で起動し、使われなくなると終了します。セッションは`~/.hermes/state.db`に保存され、{{ui:resume.choice.resume}}にもこのフォルダのHermesセッションが表示されます。使用量にはセッションの累計トークンと使用中のコンテキストが表示され、費用はHermesが知らせません。
- 質問: Hermesがclarifyツールで質問すると[質問カード](approvals.md#questions)が表示され、答えると同じリクエストが続きます。Hermesはエディタ接続（ACP）ではこのツールを既定でオフにしているため、最初のHermesリクエストの前に、アプリがHermesの設定の`platform_toolsets.acp`という1項目を`hermes config set`でオンにし、記録に残します。自分で保存したリストがある場合は変更しません。新しい会話から適用され、元に戻すにはターミナルで`hermes config unset platform_toolsets.acp`を実行します。Hermesが回答を待つのは一定時間（既定で90秒）だけです。その後はカードが閉じ、Hermesは回答なしで先に進みます。
- まだできないこと: Hermesのペインは[Mighty Orchestration](delegation.md)に参加せず、[リモートワークスペース](remote.md)では開けません。

### その他の実行設定 {#run-settings}
…ボタンをクリックすると{{ui:settings.run.title}}が開きます。
- Claude: {{ui:settings.run.maxTurnsLabel}}と{{ui:settings.run.maxBudgetLabel}}。空欄にすると上限を設けません。
- Codex: {{ui:settings.run.webSearchLabel}}の設定と{{ui:settings.run.shellNetworkToggle}}。入力欄のFastチップは、対応しているモデルとアカウントでより速く応答しますが、使用量が多くなります。

値を変更したら{{ui:settings.run.applyButton}}をクリックします。

![[composer-settings]]

### ファイルと画像の添付 {#attachments}
クリップのボタンをクリックするか、ファイルを入力欄にドラッグ&ドロップするか、{{kbd:⌘V}}で画像を貼り付けます。
- 最大8個、1ファイルあたり5 MiB、合計8 MiBまで添付できます。画像(PNG・JPEG・GIF・WebP)、PDF、テキストファイルはその形式のまま、それ以外のファイルはバイナリファイルとして渡されます。
- 送信前にプレビューして、1つずつ外せます。添付だけを送信することもできます。
- ターミナルペインには添付できません。

### 送り先 {#ask-target}
スマートフォンアプリでは、oh-my-claudecodeがインストールされたClaudeペインの入力欄に{{ui:composer.ask.menuLabel}}スイッチが表示されます。インストール済みの他のCLI(Codex、Antigravityなど)が一覧に並び、1つ選ぶと、以降のリクエストはoh-my-claudecodeの`ask`を通ってそのCLIに送られます。答えはClaudeの返信の中に戻ってきます。{{ui:composer.ask.claude}}を選ぶと、これまでどおりClaudeに送ります。MacとWindowsアプリの入力欄にはこのスイッチがなく、そこから送ったリクエストはいつもそのペインのエージェントに届きます。別のエージェントに任せるには[{{ui:composer.busySend.title}}](#queue)かキューの行のエージェント選択を使います。
- 他のCLIに尋ねている間は、添付を送れません。
- oh-my-claudecodeは、選んだCLIを承認の確認とサンドボックスをオフにして起動します。そのためそのCLIは、確認なしにこのフォルダのファイルを変更したり、コマンドを実行したりできます。Mighty Codeの権限モードと承認カードが及ぶのは、それを起動するClaudeの手順だけです。CLI を選んでいる間、入力欄にこの警告が 1 行で表示されます。
- Windowsでは、古いバージョンのoh-my-claudecodeはAntigravityへの質問を断ります。そう返ってきたら、oh-my-claudecodeを更新するか、Gemini CLIを使ってください。
- `/`で始まる文章は、そのままClaudeに送られます。

### スラッシュコマンド {#slash}
入力欄に`/`を入力すると、コマンドの一覧が表示されます。{{kbd:↑}}{{kbd:↓}}で選び、{{kbd:Enter}}か{{kbd:Tab}}で入力し、{{kbd:Esc}}で閉じます。

| コマンド (Claudeの場合) | 動作 |
|---|---|
| `/model` | {{ui:slash.builtin.model}} |
| `/permissions` | {{ui:slash.builtin.permission}} |
| `/clear` | {{ui:slash.builtin.newConversation}} |
| `/cost`, `/usage` | {{ui:slash.builtin.usage}} |
| `/plugin` | {{ui:slash.builtin.plugin}} |
| `/config` | {{ui:slash.builtin.settings}} |
| `/rename` | {{ui:slash.builtin.rename}} |
| `/help` | {{ui:slash.builtin.help}} |
| `/advisor` | {{ui:slash.builtin.advisor}} |
| `/committee` | {{ui:slash.builtin.committee}} |

CodexとAntigravityでは名前が少し異なります(例: Codexの`/new`、`/approvals`、`/status`、Antigravityの`/usage`、`/config`)。一覧にない`/名前`は、CLIのスキル、ユーザーコマンド、プラグインのコマンドとしてそのまま渡されます。Antigravityのスキルもこうして実行され、一覧にはagyに入っているスキル、`~/.gemini/antigravity-cli/skills`の自分のスキル、フォルダの`.agents/skills`が表示されます。piは`/new`、`/session`、`/settings`、opencodeとHermesは`/new`、`/usage`、`/settings`を使います。`/help`はどのエージェントでも、このペインで使えるコマンドをアプリが直接表示します。`/skills`のようなAntigravity自身の読み取り専用コマンドは、ターミナルで`agy -p "/skills"`のように別に実行したときだけ答えるため、アプリではagyのエラーで終わります。

### ほかのエージェントの意見 {#advisor}
`/advisor`は、この画面の最後の計画（あれば）、最後の結果、最後のリクエストで変わったファイルの要約を別のエージェントに送り、意見を受け取ります。`/advisor codex この計画で大丈夫？`のようにエージェントと質問を付けられます。エージェントを省くと、この画面とは別の、インストールしてログインした最初のエージェント（Claude、Codex、Antigravity、piの順）に聞きます。`/committee`は2つのエージェントに同じ質問を送り、意見を2つ並べて表示します。共通点や違いの要約はしません。

- 意見を出すエージェントは1回だけ、読み取り専用で実行されます（Claudeは計画モードと読み取りツールのみ、Codexは読み取り専用サンドボックス、Antigravityは計画モードとサンドボックス、piは読み取りツールのみ）。ファイルを変えたりコマンドを実行したりせず、セッションも残しません。opencodeとHermesは1回の実行で承認を自分で通すため選べません。
- 送る内容はサイズを減らし、秘密に見える行を削除します。答えは画面の下に意見カードとして付き（エージェント、モデル、分かれば費用）、{{ui:advisor.card.insert}}で入力欄に入れてこの画面のエージェントに反映を頼めます。費用は使用量の記録にこの画面のワークスペースとして入ります。

### ボット {#bots}
このコンピューターのエージェントペインはすべてボットです。ペインを作るとタイトルから名前（`@名前`）が自動で付き、ほかのエージェント、スケジュール、`mighty` コマンドがその名前でリクエストを送れます。名前と指示は、ペイン見出しの ··· メニューかサイドバーの右クリックの {{ui:bot.menu.edit}} で変更します。サイドバーではペインのタイトルの横に `@名前` が付きます。ボットも普通のペインなので、承認・質問カード、キュー、スマートフォン、リモートミラーはそのまま動きます。

- **ボット一覧**：サイドバーのワークスペース一覧の下にある {{ui:bot.sidebar.title}} グループに、すべてのボットが二つの分類で並びます。ワークスペースのペインのボットは {{ui:bot.sidebar.workspaceBots}}、独立ボットは {{ui:bot.sidebar.otherBots}} に入り、分類ごとに折りたためます。行ごとに状態、名前、所属ワークスペース（独立ボットは {{ui:bot.sidebar.independent}}）が表示され、クリックするとそのペインを開き、ダブルクリックするとそのボットのワークスペースを選んで上の一覧で開きます（独立ボットは除く）。グループは折りたためます。
- **独立ボット**：ワークスペースに属さず、自分のフォルダーで作業するボットです。{{ui:bot.sidebar.title}} グループの + を押し、名前、エージェント、指示を決めます。{{ui:bot.new.create}} はアプリのデータフォルダー内の `bots/名前` フォルダーで、{{ui:bot.new.pickFolder}} は自分で選んだフォルダーで作業します。独立ボットはサイドバーのワークスペース一覧には表示されません。
- **名前**：文字（どの言語でも、日本語も可）と数字の 1〜32 文字で、間に `-`、`_`、`.` を使えます（空白は不可）。大文字と小文字は区別しないので、`@Reviewer` と `@reviewer` は同じボットです。1 つのプロファイルで同じ名前を使えるのは 1 つのペインだけです。
- **指示**：ボットの役割と進め方です（16 KB まで）。新しい会話を始めるたびに最初のリクエストのあとに付けてエージェントにだけ送られ、画面のリクエストブロックには送ったリクエストだけが表示されます。変更した指示は次の新しい会話から使われます。
- **呼び出し方**：ペイン ID の代わりに `@名前` を使います。
  - `mighty agent send @reviewer "PR 12 を見て"`、`mighty agent wait @reviewer --until done`、`mighty pane read @reviewer`
  - スケジュールの heartbeat：`pane: "@reviewer"`。ほかのワークスペースのボットも呼べます。その名前のボットがない場合は新しいペインを作らず、失敗として記録します。
  - ほかのエージェント（Claude、Codex のペイン）：mighty-terminal のツール `list_bots`、`ask_bot`、`bot_answer`。`ask_bot` は送ってすぐ戻り、`bot_answer` が状態と最後の答えを返します（秘密の値は伏せる。`wait: true` なら最大 20 秒待つ）。
- **安全のルール**
  - リクエストはいつもボットペイン自身の設定と権限で実行されます。呼び出す側が権限を広げることはできません。
  - ボットはほかのボットや自分自身にリクエストできません。
  - 1 日の予算を使い切ると送りません。

### コンテキストと使用量 {#session-info}
送信ボタンの横の円形のリングは、会話がコンテキストをどれだけ使ったかを示します。クリックすると、{{ui:composer.sessionInfo.contextUsage}}、入力・出力・キャッシュのトークン、コスト、使用したモデル、セッションIDを確認できます。CLIが報告した値だけを表示し、不明な値は推定しません。

![[session-info]]

## Mighty表示 {#views}
すべてのエージェント(Claude、Codex、Antigravity、pi、opencode、Hermes)のペインは{{ui:graph.view.mighty}}表示で表示されます。リクエスト、サブエージェント、バックグラウンドタスク、結果をブロックのフローで表示します。切り替える操作はありません。

### Mightyフロー {#diagram}
Claude ペインのフローは、リクエスト → その間に起きたこと → {{ui:graph.block.result}} の順に見えます。リクエストブロックには送ったリクエストだけが入ります。回答が来るまでは、結果が来る場所に{{ui:graph.block.execution}}カードが立ち、メインセッションのモデルといま行っていること1行（Claudeが最後に言ったこと、またはそのあとに実行したコマンド）を表示します。カードをクリックすると、メッセージとツール呼び出しがすべて入った{{ui:graph.block.execution}}ブロックに展開し、回答が来るとその場所に{{ui:graph.block.result}}カードが立ちます。途中に何もない短い質問は、リクエストと結果の2つのブロックだけです。ほかのエージェントのペインは、これまでどおりリクエストブロックのすぐ下に{{ui:graph.block.execution}}ブロックがつきます。

ブロックのヘッダーは、先頭の状態の点とタイトルでどんな状態かを示します。点の色が状態です（実行中は青、ユーザーの応答待ちは黄、完了は緑、エラーは赤、停止はグレー）。{{ui:graph.block.result}}はタイトルもその色に染まり、終わった結果がひと目でわかります。誰が作業しているか（メインセッション、サブエージェント、バックグラウンドコマンド）は種類の表示の色でわかり、点にマウスを重ねると役割が表示されます。タイトルのあとにはサブエージェントの種類（Explore、自分で作ったエージェントの名前など）とバックグラウンド、フォーク、worktree の印がつき、{{ui:graph.block.execution}}ブロックにはペインで選んだ思考の強さがつきます。モデルはトークン表示と一緒に出て、まだ応答がなければ要求したモデルのあとに「要求」と書きます。自分で作ったエージェントの定義ファイルに `color` があればその色を使います。ブロックが狭いとこれらの印から先に隠れ、点にマウスを重ねると全体を見られます。

Claude ペインのフローでは、結果以外はすべて小さなカードにたたまれています。サブエージェントとバックグラウンドコマンドは実行中でも終了後でもカードで、カードもブロックと同じヘッダー（アイコン、タイトル、状態、展開ボタン）の下に2行を表示します。いま行っていること（終わったら回答の1行目）と、種類とモデル、経過時間、ツール呼び出し数、トークンです。アプリが実際に見た値だけが出て、進捗率はありません。カードをクリックすると完全なブロックに展開し、ブロックのヘッダーの{{ui:graph.card.fold}}ボタンでまたたたみます。終わったバックグラウンドのサブエージェントのカードをクリックすると、{{ui:graph.background.noticeRun}}の図にあるそのブロックが展開します。バックグラウンドで実行したコマンド（CI待ち、アプリのビルド、開発サーバーなど）も同じ列に灰色のカードとして立ち、説明とコマンド、そのコマンドが最後に出力した行、実行中の時間か終了状態と終了コードを表示します。

Claudeがやることリストを作ると、リクエストの下に始めたステップが1つずつカードとして立ちます（ステップ1 → ステップ2 → …）。そのステップの間に起動したサブエージェントとコマンドはそのステップの右に並び、カードに `↳ 2. ノート作成` のようにどのステップの作業かを示す1行が加わります。ステップのカードをクリックすると、そのステップの間の記録が展開します。ステップのないリクエストでは、サブエージェントがリクエストの下の1列に並び、その上の枝にいくつにどのモデルで作業を任せ、いくつがまだ実行中かが出ます。アプリはツールを使うリクエストごとにClaudeにまずやることリストを作らせるので、短い回答でなければほとんどのリクエストにステップができます。

Claude ペインのフローの下、入力欄の上には{{ui:graph.log.title}}の帯があります。ヘッダーの一行はいまの状態です。メインセッションのモデルと状態、サブエージェントのうちいくつが実行中か、バックグラウンドのコマンドがいくつ動いているかが、役割の色で表示されます。帯を開くと、最新のリクエストで最近起きたことが5行、時刻、誰が（メインセッションのモデルまたはサブエージェントの名前）、何を（ツール呼び出し、サブエージェントの開始と終了、質問、{{ui:graph.block.compact}}、通知の到着）の順に表示されます。行をクリックすると、フローがそのブロックへ移動します。右端のボタンで折りたたむと状態の行だけが残り、折りたたんだかどうかはペインごとに記憶されます。

バックグラウンドで始めたサブエージェントとバックグラウンドコマンド（タスク）は、実行中はリクエストのブロックとしては描かれず、下の{{ui:graph.progress.title}}パネルの{{ui:graph.progress.background}}のまとまりにあります。Claudeペインのフローでは、これらは上のカードとしても表示されます。タスクが終わって通知がClaudeに届くと、Claudeの応答ごとに新しい{{ui:graph.background.noticeRun}}の図ができ、終わったタスクごとに1つのブロックが、種類の行（サブエージェントかバックグラウンドコマンドか、終わり方）、コマンド、出力の最後の数行、サブエージェントの報告を示します。{{ui:graph.view.timeline}}ではこの図が新しい行として、{{ui:graph.view.structure}}では最後に新しいカード（ボード）として付きます。スマートフォンのMighty表示とパイプラインも同じです。

すべてのエージェントのペインのフローの左上には{{ui:graph.progress.title}}パネルが浮かんでいます。画面に見えるリクエストの番号と状態、リクエストの内容が出て、エージェントが計画を知らせるとステップごとに完了、進行中（時間が流れる）、未着手が出ます。ステップはClaudeのやることリスト、Codexの計画（`update_plan`）、opencode・Hermesの計画、Geminiのチェックリスト（task.md）から来ます。{{ui:menu.settings}} › {{ui:settings.nav.general}}の{{ui:settings.agents.checklistToggle}}（初期状態でオン）は、Gemini、Codex、opencode、Hermesに作業中の計画を残すよう頼みます。計画が届かないリクエストと、計画機能のないpiは、エージェントがしていること（ファイルの読み取り・書き込み・編集、コマンド、検索、サブエージェント）を1行ずつステップとして表示します。リクエストがバックグラウンドタスク（コマンド、バックグラウンドのサブエージェント）を起動していれば、一番下の{{ui:graph.progress.background}}のまとまりに実行中の数と全体の数、タスクごとに1行（状態とかかった時間）が出ます。実行中のものから100行まで入り、パネルの中でスクロールして見ます。それより多い分は数でまとめます。新しいリクエストが始まるとそのリクエストを表示し、自分でスクロールすると画面に見えるリクエストに変わります。最終結果が届くと、フローはその{{ui:graph.block.result}}ブロックへなめらかに移動します。そのときフローを自分で動かしている最中や、別のブロックへ移動している途中であれば、そのままにします。リクエストの行をクリックするとそのリクエストへ、ステップの行をクリックするとそのステップのカードへ移動します（ステップのカードはClaudeペインのフローにだけあります）。ヘッダーのボタンでたたむと、ヘッダーとリクエストの行だけが残ります。

- 何もない場所をドラッグするかスクロールして、フローを動かします。拡大・縮小ボタンで50〜150%の倍率で表示できます。
- ブロックをクリックすると、そのブロックの中だけがスクロールします。何もない場所をクリックするか{{kbd:Esc}}を押すと、選択が解除されます。
- ブロックの右下の角をドラッグするとサイズが変わります。右クリックすると{{ui:graph.block.resetSize}}があります。サイズはセッションごとに保存されます。
- 一番上までスクロールすると、{{ui:graph.history.load}}で以前のリクエストを10件ずつ読み込みます。読み込んだ履歴は閲覧専用です。
- ClaudeとCodexのペインでは、ヘッダーのパズルのボタンで、インストール済みのプラグインとマーケットプレイスを開きます。

![[agent-mighty]]

![[agent-mighty-overview]]

### 記録 {#timeline}
Mighty表示の上部にある{{ui:graph.view.diagram}} | {{ui:graph.view.timeline}}の切り替えで、リクエストを時系列の一覧として表示できます。リクエストの行の次に{{ui:graph.block.execution}}の行が、その次にサブエージェントの行が続きます。

![[agent-timeline]]

### パイプライン {#structure}
同じ切り替えで{{ui:graph.view.structure}}を選ぶと、リクエストをかんばん形式のパイプラインで表示できます。
- 表示は一度に一つです。スタイルのないペインではリクエスト一つ、スタイルを使うペインではパイプライン一つです。パイプラインはフェーズが前にだけ進むリクエストのまとまりで、前のリクエストより前のフェーズに戻ったリクエストから新しいパイプラインが始まります。
- 上部の ◀ {{ui:graph.structure.board.previous}}と ▶ {{ui:graph.structure.board.next}}で前後に移動します。その間の「リクエスト 3 / 7」のようなタイトルを押すと一覧が開き、番号、タイトル、状態、ステップを見てすぐに選べます。
- その下に状態（スタイルを使うペインではパイプライン名も）が表示されます。ステップは各列の上のチップで、チップは線でつながっています。スタイルのないペインでは{{ui:graph.structure.column.request}} → {{ui:graph.structure.column.execution}} → {{ui:graph.structure.column.result}}、スタイルを使うペインではそのモードのフェーズです。
- 内容はステップごとに横に並んだ列に入ります。列の見出しにはステップのチップとカードの数があります。スタイルのないペインでは、{{ui:graph.structure.column.request}}列に送ったリクエスト、{{ui:graph.structure.column.execution}}列に進み具合（最新のメッセージとツール呼び出し）と状態のカードとサブエージェントごとに一枚のカード、{{ui:graph.structure.column.result}}列に結果が置かれます。スタイルを使うペインでは、フェーズの列ごとにそのフェーズのリクエストカード（リクエスト、実行の要約、結果の1行）と、その下にサブエージェントのカードが置かれます。入れ子のサブエージェントは字下げされ、どのブロックの下かが書かれます。
- 状態は色でもわかります。各列の背景にはそのステップの状態の色が薄く敷かれ（完了は緑、実行中と通知待ちは青、ユーザーの応答待ちは黄、エラーは赤、停止はグレー）、今のステップは最新のリクエストの状態に合わせ、まだ来ていないステップは背景のままです。カードは状態の文字の前の点が同じ色で、背景は列より濃く色づきます。実行中のカードは動く枠線、待機中のカードは黄色の枠線のままで、状態の文字もそのまま表示されます。
- 長い文章は折りたたまれていて、{{ui:graph.structure.more}}を押すとその場で開きます。カードの見出しにある小さなボタンを押すと、フローでそのブロックを表示します。
- 表示は最新のものから開き、そのままにしておくと新しいリクエストやパイプラインに付いていきます。前のものを見ている間に新しいものが来ても位置はそのままで、▶ の横に{{ui:graph.structure.board.newRequest}}または{{ui:graph.structure.board.newPipeline}}の印が付きます。印を押すと最新のものへ移ります。
- 列はそれぞれ別々に動きます。列の見出しは上に残り、カードだけが上下に動きます。列の上でトラックパッドやマウスホイールを上下にスクロールするか、何もない場所をドラッグすると、その列だけが動きます。列が画面に収まらないときは、横にスクロールするかドラッグしてパイプラインを横に動かします。
- 最新のものでは、終わりを見ている列が新しい内容に付いて下がります。自分で動かした列や文章を広げた列はその位置に留まり、その列の終わりに戻るとまた付いていきます。新しいリクエストが来るとすべての列が終わりを表示し、以前のものはどの列も一番上から開きます。Windowsアプリではまだパイプライン全体が一緒に動きます。

![[agent-structure]]

### 結果カード {#result}
リクエストが終わると、結果カードに結果がまとめられます。Claudeの応答が終わるとリクエストは完了になります。そのリクエストが始めたバックグラウンドタスク（終わったサブエージェントにメッセージを送って再開させたものを含む）がまだ実行中か、終わってもClaudeがその通知にまだ応答していないあいだは、結果カードが待機色の{{ui:graph.block.awaitingNotice}}ブロックになります。これまでの応答の下に、タスクごとに{{ui:plan.background.awaitingNotice}}または{{ui:plan.background.noticeDelivered}}と、「通知 1/2」のような受け取った通知の数が表示されます。このあいだMighty Petとペインのヘッダーは{{ui:graph.state.awaitingNotice}}を表示します。すべての通知に応答すると{{ui:graph.block.result}}に変わります。バックグラウンドタスクの通知を受けてClaudeがまた応答すると、その応答はリクエストの結果には続かず、応答ごとに新しい図が開きます([バックグラウンドタスク](approvals.md#background))。同じリクエストのほかのバックグラウンド作業がまだ実行中なら、その応答のカードのタイトルは「これまでの結果 · バックグラウンド作業 N 件を待機中」となって残りの数を示します。結果にファイルが含まれていれば、{{ui:graph.resultFiles.openButton}}ボタンで一覧を開きます。ブロック内のファイルパスをクリックするとプレビューが表示されます。最新の結果カードはウインドウのサイズに合わせて表示され、サイズを手動で変更した場合は{{ui:graph.result.fitToWindow}}ボタンが表示されます。 リクエストが変更したソースやMarkdownのファイルなら、プレビューの見出しに{{ui:files.changes.show}} | {{ui:files.changes.full}}の切り替えが付き、{{ui:files.changes.show}}から表示されます。変更された行だけを前後3行とともに、古い行番号と新しい行番号を付けて表示し、追加した行は緑、削除した行は赤の背景です。ソースは言語に合わせて色分けされ、行の中で変わった単語が強調されます。上の行の{{ui:files.changes.unified}} | {{ui:files.changes.split}}の切り替えで、1 列の表示と、古い内容と新しい内容を左右に並べる表示を切り替えます(選んだ表示は記憶されます)。全体表示のソースも同じように色分けされます。上の行が何と比べたかを示します。結果のファイル一覧やブロックから開くと、そのリクエストがファイルを最初に変更する前の内容（{{ui:files.changes.baseRun}}）、その元がなければ最後のコミットです。変更がなければ切り替えはなく、ファイル全体だけが表示されます。

同じリクエストのバックグラウンド作業のあとでClaudeがもう一度答えると、前の結果カードのタイトルは{{ui:graph.block.resultPartialPlain}}になり、その応答を開いた作業の名前が付きます(例: 「途中の結果 · Macビルド」)。リクエストの本当に最後の結果だけが{{ui:graph.block.result}}です。最後の結果カードには{{ui:graph.resultFiles.rollUpTitle}}の一覧が付きます。アプリはリクエストの開始時、チェックリストのステップが終わるたび、バックグラウンド作業の応答が終わったとき、実行が終わったときに作業フォルダーのgitスナップショットを取り、その間に変わったファイルをステップごとにまとめて表示します。エージェントが編集ツールで直してもコマンドで直しても同じで、Codex・Gemini・opencode・Hermes・piのペインも同じです(ツール呼び出しひとつひとつがステップのペインでは、実行全体がひとまとまりです)。{{ui:graph.rollUp.bySteps}} | {{ui:graph.rollUp.byFiles}}でステップ別のまとまりとファイル別の一覧を切り替えます。まとまりが5つ以上あると最後の2つだけを開いておき、まとまりごとに5行まで表示し、残りは「さらにN件表示」で開きます。ひとつのフォルダーにファイルが3つ以上あるとフォルダー名の下にまとめ、ファイルが20を超えると検索欄が出ます。ファイル名をクリックするとプレビューが開き、右の+/−の数をクリックすると、そのステップでそのファイルがどう変わったかをすぐ下に表示します。回答に出てきたファイルはその下に続きます。スナップショットはgitリポジトリ内のフォルダーでだけ取られ、リポジトリの`.git`には何も書かず、アプリのデータフォルダーの`snapshots`に保存し、14日間使われなかったものは削除します。

![[result-card]]

### 検証レシート {#verify-receipt}
[ターン終了時の検証](delegation.md#verify)を行ったリクエストの結果カードの下にレシートが付きます。合格・失敗と何回目の確認か、失敗なら止まった理由、コマンドごとの合否とかかった時間（失敗なら終了コードかタイムアウト）、ブラウザ点検のページ・コンソールエラー・例外・失敗したリクエストの数とスクリーンショットのボタンが表示されます。修復のあいだは最後の修復リクエストの結果カードへ移ります。

- {{ui:verify.card.autoRepair}}: オフにすると、このペインでは失敗しても修正を頼まず結果だけを残します。
- {{ui:verify.card.runAgain}}: 今すぐ一度確かめます（修正は頼みません）。確かめているあいだは {{ui:verify.card.stop}} ボタンになり、押すと今のコマンドが終わったあと止まります。
- まだ確かめていない最新の結果には {{ui:verify.card.run}} ボタンがあります。スマートフォンのペイン画面にも最新の検証結果が 1 行で表示されます。

### 監査ログ {#audit}
結果カードのヘッダーにある {{ui:turnAudit.open}} ボタン（またはヘッダーを右クリックしたメニュー）を押すと、そのリクエストが行ったことを時系列で 1 行ずつ確認できます。自動編集モードで長く実行した後に、何をしたかを確かめるときに使います。

- 実行したコマンド、編集したファイルとリクエスト終了時に変更されていたファイル（追加・削除した行数）、開いたアドレスと Web リクエスト、計画カードの判断と拒否したツールリクエスト、サブエージェントと委任で生まれた子ペイン、[検証](#verify-receipt)の結果が表示されます。サブエージェントが行ったことにはその名前が付きます。
- ファイルの読み取りや検索は一覧から外し、件数だけを下に記します。上のボタンで種類ごとに絞り込めます。
- アプリがすでに保存した実行記録から作るだけで、新たに記録はしません。そのため、確認なしで許可されたツール呼び出しは承認の行として表示されず、拒否したリクエストと計画カードの判断だけが承認の行になります。
- {{ui:turnAudit.copy}} ボタンまたはメニューの {{ui:turnAudit.copyMarkdown}} は、一覧を Markdown でコピーします。コピーするときに秘密の値とホームフォルダーのパスは取り除きます。

### リクエストの変更をレビュー {#request-changes}
Claude ペインのリクエストが Edit・Write・MultiEdit でファイルを変えると、変える直前の内容が保管されます。そのリクエストが終わると結果カードのヘッダーに {{ui:runChanges.openButton}} ボタンが現れ、押すと {{ui:runChanges.title}} が開きます。左にはリクエストが変えたファイルと追加・削除した行数、右には選んだファイルをリクエスト前の内容と比べた変更が表示されます。

- 変更の行をクリックすると、その行にコメントを残せます。「コメントN件を入力欄に入れる」を押すと、ファイルと行の位置が付いた一つの追加リクエストとして入力欄の文の後ろに入ります。すぐには送信しません。
- ファイルにチェックを入れて「選択したN件を元に戻す」を押し、確認すると、そのファイルはリクエスト前の内容に戻ります。リクエストが新しく作ったファイルはゴミ箱(Windows ではごみ箱)に移します。ファイルの今のエンコーディングと権限はそのままで、リクエストの後に自分で直した内容も一緒に消えます。
- 後のリクエストも同じファイルを変えていたら、そちらを先に元に戻す必要があります。なくなったファイル、テキストとして読めないファイル(1 MiB 超・バイナリ)、すでにリクエスト前と同じファイルは元に戻しません。
- 変更された部分(hunk)ごとにヘッダーに {{ui:hunks.accept}} と {{ui:hunks.discard}} があります。{{ui:hunks.discard}} はその部分だけをリクエスト前の内容に戻し、ほかの行はそのままにします。ファイルのエンコーディング、改行(CRLF・LF)、最後の改行の有無、権限は保ちます。最後の部分まで破棄してリクエスト前と同じになると、ファイルを元に戻したのと同じになり、リクエストが作ったファイルはゴミ箱に移します。{{ui:hunks.accept}} はファイルを変えず、レビュー済みにするだけで、アプリを再起動しても残ります。受け入れた部分は薄く折りたたまれ、ヘッダーを押すとまた開きます。
- 左の一覧にファイルごとの処理済みの部分数(n/m)が表示され、すべての部分を受け入れるか破棄したファイルは {{ui:hunks.reviewed}}(✓)になります。{{ui:hunks.acceptRest}} はまだ処理していない部分をすべて受け入れます。すべてのファイルがレビュー済みになると、そのリクエストは[レビューキュー](layout.md#review)から外れます。
- レビュー画面にフォーカスがあるとき、`]`・`[` で次・前の部分、`a` で受け入れ、`x` で破棄です。
- リクエストの後にその部分が変わっていたら破棄しません({{ui:hunks.refusalShort.conflict}})。上に行が足されて数行ずれただけなら、その位置を見つけて破棄します。後のリクエストが同じ行を変えていたらその部分だけを止め、離れた部分は破棄できます。後のリクエストがそのファイルの元の内容を保管できなかったときは、ファイル全体を止めます。
- worktree ペイン、子ペイン、比較実行の変更表示でも同じボタンとキーを使います。そこでは基準ブランチと分かれたコミットの内容が基準です。
- ペインごとに直近 20 件のリクエストの元の内容を保管します。シェルコマンドが変えたファイル、ほかのエージェントのペイン、リモートワークスペースのペインにはこのボタンがありません。
- 変更の行を右クリックすると、そのファイルのその行を外部エディタで開きます。ヘッダーのボタン({{ui:editor.open}})は選んだファイルを開きます([エディタで開く](terminal-files-browser.md#open-in-editor))。
