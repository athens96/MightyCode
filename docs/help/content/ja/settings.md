---
title: 設定
order: 8
section: settings
---
{{ui:menu.settings}}({{kbd:⌘,}})か、サイドバー下部の設定ボタンで開きます。左側でタブを選び、{{ui:settings.closeButton}}ボタンか{{kbd:Esc}}で閉じます。最後に表示したタブが記憶されます。タブは{{ui:settings.nav.general}}、{{ui:settings.nav.models}}、{{ui:settings.nav.styles}}、{{ui:settings.remote.navTitle}}、{{ui:settings.nav.companion}}、{{ui:settings.nav.tools}}、{{ui:settings.nav.about}}の順に並んでいます。

## 一般 {#general}
{{ui:settings.nav.general}}タブの{{ui:settings.display.sectionTitle}}で、アプリの見た目を変更します。
- {{ui:settings.display.themeLabel}}: {{ui:settings.display.themeDarkWindows}}または{{ui:settings.display.themeLightWindows}}。サイドバー下部のボタンでも切り替えられます。
- {{ui:settings.display.languageLabel}}: アプリの言語。[言語を変更する](getting-started.md#language)を参照してください。

![[settings-general]]

### 文字サイズとフォント {#text}
同じタブの {{ui:settings.text.sectionTitle}} で、アプリ全体の文字を変更します。
- {{ui:settings.text.sizeLabel}}：85% から 150% の間で選びます。会話、入力欄、サイドバー、設定、ターミナルが一緒に大きく・小さくなります。
- {{ui:settings.text.uiFontLabel}}：サイドバー、メニュー、ボタン、設定など、アプリ画面のフォントです。このコンピュータにインストールされているフォントから選び、{{ui:settings.text.fontSystem}} は OS の標準フォント（Mac では macOS のフォント、Windows では Segoe UI）です。
- {{ui:settings.text.transcriptFontLabel}}：エージェントの返答、自分が送ったメッセージ、入力欄、Mighty のカードとボードの本文のフォントです。{{ui:settings.text.sameAsInterface}} にすると画面のフォントに合わせます。
- {{ui:settings.text.codeFontLabel}}：コードブロック、インラインコード、ツールの出力とコマンド行、ターミナルペインのフォントです。等幅フォントだけが表示されます。インストールされている等幅フォントのほか、JetBrains Mono、D2Coding、Fira Code、Cascadia Code・Cascadia Mono、Consolas などのコーディング用フォントもインストールされていれば表示されます。いちばん上の標準は、Mac ではコードに SF Mono、ターミナルに Menlo を、Windows では Cascadia Mono（ない場合は Consolas）を使います。Mac で選んだフォントが削除されると、ターミナルは Menlo に戻ります。開いているターミナルにもすぐに反映されます。パス、バージョン、件数などアプリ画面の等幅の文字は、システムの等幅フォントのままです。
- それぞれのフォントの下に、そのフォントで書いた 1 行のプレビューが表示されます。
- {{ui:settings.text.reset}} ボタンで、サイズと 3 つのフォントを最初の状態に戻します。

Mac では表示メニューの {{ui:menu.textBigger}}（{{kbd:⌘=}}）、{{ui:menu.textSmaller}}（{{kbd:⌘-}}）、{{ui:menu.textActualSize}}（{{kbd:⌘0}}）で、Windows では {{kbd:Ctrl+=}} / {{kbd:Ctrl+-}} / {{kbd:Ctrl+0}} でもサイズを変更できます（Windows でターミナルペインにカーソルがあるときは、これらのキーはターミナルが使います）。変更はすぐに反映され、アプリを開き直してもそのままです。

### エージェントペイン {#agents}
同じタブの{{ui:settings.agents.sectionTitle}}には、エージェントペインに関する設定とキーの案内があります。
- {{ui:settings.display.statusLineToggle}}: Claudeの設定にあるステータスラインのコマンドを実行し、入力欄の下に表示します。
- {{ui:settings.delegation.toggle}}: 初期状態でオンです。Claudeペインが専用のworktreeで子ペインを開けるようになり、実行中のClaudeペインで{{kbd:⌘Enter}}を押すとリクエストが新しい子ペインで実行されます。[Mightyオーケストレーション](delegation.md#enable)を参照してください。
- {{ui:settings.agents.checklistToggle}}: 初期状態でオンです。オンのあいだは、Gemini、Codex、opencode、Hermesに作業中の計画（チェックリスト）を残すよう短い指示を添え、進行状況パネルに実際の計画のステップが出ます。ペインのリクエストには入力した内容だけが残ります。オフのときや計画が届かないときは、エージェントがしていることがステップとして出ます（[マイティ表示](agent-pane.md#diagram)）。
- {{ui:settings.editor.label}}: {{ui:editor.open}}が起動するエディタです。自動(見つかった最初のもの)、このコンピュータで見つかったエディタ、{{ui:settings.editor.system}}から選びます。設定を開くたびに探し直します([エディタで開く](terminal-files-browser.md#open-in-editor))。
- その下にキーの案内があります。{{kbd:Enter}}で送信し、{{kbd:Shift+Enter}}で改行します。タブをドラッグしてほかのタブとまとめたり、端へ移して分割したりします。コマンド実行ペインは、Macではログインシェル、WindowsではPowerShell(ない場合はcmd)を開く対話型ターミナルです。
- Issue トラッカー(MightyWork、GitHub、Linear、Jira、GitLab)の接続は設定ウィンドウではなく、ワークスペースごとにサイドバーのワークスペースメニューの {{ui:integrations.menu.settings}} で行います。[Issue から始める](delegation.md#issues)を参照してください。

### バックグラウンド作業 {#background}
同じタブの{{ui:daemon.settings.sectionTitle}}で{{ui:daemon.settings.toggle}}をオンにすると、アプリを終了したり自動更新で再起動したりしても、エージェントの実行とターミナルが止まりません。最初はオフです。
- オンにすると、アプリは同じ実行ファイルをウィンドウのないバックグラウンドのデーモンとして起動します(`MightyCode --daemon`)。データフォルダーごとに 1 つだけ実行され、その後に始めた Claude の実行とターミナルペインのシェルはデーモンが受け持ちます。画面、履歴、承認カードはこれまでどおりアプリが描きます。
- 実行中の作業があるときにアプリを終了すると、{{ui:daemon.quit.keep}}と{{ui:daemon.quit.stopAll}}から選びます。自動更新での再起動では常に続けます。
- アプリを再び開くと続きから始まります。Claude ペインはアプリを閉じていた間の出力から続けて表示し、その間に承認・質問待ちになったカードも再び表示します。ターミナルペインは開いたときにシェルと画面の履歴(直近 2MB)が戻ります。デーモンは実行ごとに出力の直近 8MB を保持します。
- 状態の行には、デーモンの PID、実行中の作業の数、保持中の出力のサイズが表示されます。{{ui:daemon.settings.stop}}ボタンはデーモンと中の作業をすべて終了します。実行中の作業も接続中のアプリもなければ、デーモンは 10 分後に自動で終了します。

> [!note]
> 続くのは Claude エージェントの実行とターミナルペインです。ほかのエージェント(Codex、Gemini など)の実行はデーモンで動いていても、アプリを閉じると止まります。続きから再接続した Claude の実行は、そのターンが終わるまで Mighty との連携(委任ツール、アクティビティ表示)を使えないことがあります。デーモンが異常終了すると中の作業も終わり、ペインは停止になります。更新でデーモンとアプリのバージョンが異なる場合、アプリは接続せず、以前のデーモンの作業を停止するか確認します。デーモンに接続できるのはこのコンピューターの同じユーザーだけです(Mac は権限 0600 のソケット、Windows は現在のユーザーだけが開ける名前付きパイプ、データフォルダーの `daemon/token` のトークン)。

### ウィンドウなしで実行 (サーバー) {#server}
ウィンドウを開かずに Mighty Code 全体(スマートフォン連携、リモートミラー、`mighty` コマンド、スケジュールとトリガー、バックグラウンドのデーモンとの接続)を実行できます。ログインしたままの Mac や PC をスマートフォンやスクリプトから使うときに向いています。
- `mighty server start` はアプリをウィンドウなしで起動し(`MightyCode --server`、デーモンの `--daemon --headless` とは別です)、応答するまで待ってから状態を JSON で表示します。Mac では Dock アイコンとメニューバーが表示されません。
- `mighty status`、`mighty agent new`、`send`、`wait` などのコマンドはそのまま使えます。`mighty server status` はウィンドウの有無、バージョン、PID、稼働時間を表示します。
- ウィンドウが必要なときはアプリをもう一度開きます(Mac は Dock や Finder から、Windows はアプリをもう一度起動)。同じデータフォルダーのアプリが 2 つ動くことはなく、実行中のサーバーがウィンドウを開きます。`mighty server show` も同じです。このウィンドウを閉じてもサーバーはウィンドウなしで実行を続け、⌘Q を押すと{{ui:server.quit.closeWindow}}と{{ui:server.quit.stop}}から選びます。
- `mighty server stop` は通常どおりアプリを終了します。{{ui:daemon.settings.toggle}}がオンならデーモンの作業は続け、`--stop-all` を付けると一緒に停止します。Mac の SIGTERM と Windows のコンソールを閉じる操作も同じ終了です。
- {{ui:daemon.settings.sectionTitle}}の{{ui:server.settings.loginToggle}}をオンにすると、ログイン時にサーバーとして起動します(Mac は `~/Library/LaunchAgents` の LaunchAgent、Windows は現在のユーザーの Run レジストリキー)。最初はオフで、次回のログインから有効になります。

> [!note]
> ターミナルペインのシェルは、ウィンドウがそのペインを初めて表示したときに起動します(ウィンドウ付きで起動したときと同じです)。ウィンドウを閉じても実行中のシェルは動き続け、`mighty pane read` で読めます。スマートフォンの接続承認のように確認が必要なことが起きると、ウィンドウが開きます。

## モデル {#models}
{{ui:settings.nav.models}}タブの{{ui:settings.phaseModels.sectionTitle}}で、作業の段階ごとに使うモデルをこのコンピュータ全体で設定します。段階は{{ui:settings.phaseModels.phase.planning}}、{{ui:settings.phaseModels.phase.execution}}、{{ui:settings.phaseModels.phase.review}}、{{ui:settings.phaseModels.phase.subagents}}です。
- {{ui:settings.phaseModels.defaultOption}}のままにすると、CLIが決めます。
- {{ui:settings.phaseModels.paneModel}}を選ぶと、ペインのモデルチップで選んだモデルを使います。
- 一覧にないモデル名は、{{ui:settings.phaseModels.registeredTitle}}で{{ui:settings.phaseModels.addButton}}します。そのモデルが思考レベルに対応している場合は、{{ui:settings.phaseModels.supportsEffortLabel}}をオンにします。

![[settings-models]]

## スタイル {#styles}
{{ui:settings.nav.styles}}タブでスタイルの一覧を管理します。ファイルからスタイルを登録し、内容を確認して許可したりブロックしたりします。一覧が合わないときは{{ui:settings.styles.rescanButton}}ボタンをクリックします。スタイルのID、ハッシュ、件数は、名前にポインタを合わせると表示されます。詳しくは[スタイル](styles.md#custom)にあります。

![[settings-styles]]

## リモート接続 {#remote}
{{ui:settings.remote.navTitle}}タブでは、別のコンピュータ(Mac・Windows)やスマートフォンからこのコンピュータのワークスペースを使えるようにしたり、別のコンピュータのワークスペースを開いたりします。いちばん上の案内の下にタブが3つあります。
- {{ui:settings.remote.shareTitle}}: 上部の状態行の横にある{{ui:settings.remote.startSharing}} / {{ui:settings.remote.stopSharing}}ボタンで共有をオン・オフします。{{ui:settings.remote.pickWorkspaces}}を選び、下の{{ui:settings.remote.methodsTitle}}で{{ui:settings.remote.viaPairing}}と{{ui:settings.remote.viaTailscale}}の行を確認します。[このコンピュータを共有](remote.md#share)を参照してください。
- {{ui:settings.remote.connectTitle}}: ペアリングリンクかTailscaleのアドレスで別のコンピュータに接続し、下の{{ui:settings.remote.connectionsTitle}}一覧で接続を管理します。[別のコンピュータに接続](remote.md#connect)を参照してください。
- {{ui:settings.remote.phoneTab}}: スマートフォンを接続します。下の[スマートフォン](#mobile)を参照してください。

{{platform:windows}} {{ui:settings.remote.shareTitle}}タブと{{ui:settings.remote.phoneTab}}タブの下に{{ui:settings.remote.firewallTitle}}があります。[Windowsファイアウォール](remote.md#firewall)を参照してください。

### スマートフォン {#mobile}
{{ui:settings.remote.navTitle}} › {{ui:settings.remote.phoneTab}}タブで、スマートフォンからの接続を許可し、QRコードでスマートフォンを接続し、接続済みのデバイスを管理します。リレーを別に用意する必要はありません。[モバイルリモート](phone.md#pair)を参照してください。たたまれた{{ui:settings.advancedTitle}}には、自分で立てたリレーを使うときの{{ui:settings.mobileRemote.relayLabel}}欄({{ui:settings.mobileRemote.applyButton}}ボタン、下に既定のアドレス)と{{ui:settings.mobileRemote.legacyAppsToggle}}スイッチがあります。どちらかを既定値から変えていれば、最初から開いています。

画面の表示と操作は現在一時停止しているため、設定にその項目はありません([リモート画面](phone.md#screen))。

## Mighty Petと通知 {#companion}
{{ui:settings.nav.companion}}タブで、Mighty Petと完了通知を設定します。
- {{ui:companion.settings.enabled}}: 画面上を動き回るアライグマのMighty Petを表示します。Mighty Petはドラッグして移動でき、クリックすると吹き出しが開閉します。
- {{ui:companion.settings.task}}: 吹き出しに、リクエスト、現在の作業、経過時間を表示します。吹き出しから許可({{ui:permission.allowOnce}} / {{ui:permission.deny}})に答えることもできます。承認・質問・プランの吹き出しの記号ボタンは、リクエスト全体を見たり回答を自分で書いたりできるよう、そのペインを開きます。
- 吹き出しの上には、そのペインの最新リクエストの{{ui:graph.progress.title}}カードが付きます。ペインの進行パネルと同じ内容で、リクエスト番号、終わったステップ数、状態、すべてのステップとバックグラウンド作業が、それぞれの経過時間とともに表示されます。8行を超えるとカードの中でスクロールでき、最初はいまのステップが見える位置で開きます。カードの見出しをクリックすると見出しだけに折りたたまれ、もう一度クリックすると開きます。折りたたんだ状態は次にアプリを開いても保たれます。ステップの行をクリックすると、そのペインが開きます。ステップもバックグラウンド作業もないリクエスト、吹き出しが承認・質問・プランを表示している間、吹き出しが閉じているときはカードは表示されません。
- {{ui:companion.settings.pet}}: ペットを選びます。{{ui:companion.settings.import}}でほかのペットを追加し、{{ui:companion.settings.reload}}でペットの一覧を読み直し、{{ui:companion.settings.resetPosition}}でMighty Petの位置と吹き出しのサイズを最初の状態に戻します。
- {{ui:companion.settings.motion}}: 動きを減らします。
- {{ui:companion.settings.macNotifications}}(Windowsでは{{ui:windows.notifications.toggleLabel}}): リクエストが正常に終わると、Macの通知を送ります。通知をクリックすると、そのペインが開きます。通知にはリクエストや結果の内容は含まれません。
- {{ui:terminalNotifications.toggle}}: ターミナルペインや CLI が送った通知とベルを、そのペインに表示します。OS の通知は、アプリが背面にあるときか別のペインを見ているときだけ送ります。最初はオンです([通知とベル](terminal-files-browser.md#terminal-signals))。
- その下にOSの通知の許可状態が表示され、{{ui:windows.notifications.settingsButton}}ボタンでシステムの通知設定を開きます。

![[settings-companion]]

![[companion-pet]]

## 自動化とコマンドライン {#automation}
{{ui:settings.nav.companion}} タブの {{ui:automation.settings.sectionTitle}} ボックスで、エージェントペインの状態が変わったときに知らせる先を決めます。依頼の文面、ファイルの内容、トークンはどこにも送りません。
- {{ui:automation.settings.events}}: {{ui:automation.event.started}}、{{ui:automation.event.blocked}}、{{ui:automation.event.done}}、{{ui:automation.event.failed}}、{{ui:automation.event.stopped}} から選びます。最初は承認・質問待ち、完了、失敗がオンです。Webhook と状態コマンドの両方がこれに従います。
- {{ui:automation.webhooks.title}}: Slack、Discord、ほかの HTTPS アドレスを五つまで追加します。アドレスはシステムのキーチェーン（Windows は資格情報マネージャー）に保存し、画面には一部だけ表示します。送るのはタイトル、状態、ワークスペースとペインの名前だけです。{{ui:automation.webhooks.test}} ボタンでテストメッセージを送ります。同じペインの同じ状態は 5 秒に一度、アドレスごとに 1 分に六回まで送り、サーバーエラーのときはもう一度送ります。
- {{ui:automation.hook.title}}: ペインが選んだ状態になると、そのペインのフォルダーでこのコマンドを実行します。状態は引数ではなく環境変数で渡します: `MIGHTY_EVENT`、`MIGHTY_STATE`、`MIGHTY_PREVIOUS_STATE`、`MIGHTY_REASON`、`MIGHTY_PANE_ID`、`MIGHTY_PANE_TITLE`、`MIGHTY_AGENT`、`MIGHTY_WORKSPACE_ID`、`MIGHTY_WORKSPACE_NAME`、`MIGHTY_WORKSPACE_PATH`。20 秒で止めます。
- {{ui:automation.api.title}}: 実行中のアプリを `mighty` コマンドで操作できるかを表示します。{{ui:automation.api.rotate}} ボタンはトークンを替え、開いていた接続を切ります。

リポジトリごとに状態コマンドを置くには `.mighty.local.yaml` に書きます。[ほかの設定と同じく](delegation.md#mighty-yaml)承認した後にだけ実行され、共有の `.mighty.yaml` には書けません。

```yaml
hooks:
  command: ./scripts/notify.sh
  on: [blocked, done, failed]   # started, blocked, done, failed, stopped
  timeoutSeconds: 20            # 1-300
```

### mighty コマンド {#cli}
Mac では {{ui:menu.workspace}} メニューの {{ui:shellCommand.install}} で `~/.local/bin/mighty` を入れます。`mighty`、`mighty フォルダー`、`mighty "依頼"` はこれまでどおりフォルダーを開きます。コマンドと同じ名前のフォルダーは `mighty ./status` のように開きます。下のコマンドは実行中のアプリに接続し、JSON で答えます。0.6.0 より前に入れた場合は一度入れ直してください。Windows では `MightyCode.exe --cli status` のように使います。

```sh
mighty status                                   # ワークスペース、ペイン、状態、注意表示、worktree、ポート範囲
mighty workspace list
mighty pane list --workspace <id>
mighty pane read <pane> --lines 100
mighty agent new --workspace <id> --agent codex --worktree
mighty agent send <pane> "テストを直して"
mighty agent wait <pane> --until done --timeout 600
mighty pane wait-output <pane> --regex "passed|failed" --timeout 300
mighty events --type pane.state
mighty server start                             # ウィンドウなしで起動 (status, show, stop [--stop-all])
```

- ペイン id はアプリが使うものそのままで、変わりません。状態は `idle`、`running`、`blocked`（承認・質問待ち）、`done`、`failed`、`stopped` です。
- `agent send` は入力欄から送るのと同じです。実行中なら待ち行列に入ります。`-` を渡すと標準入力から読みます。
- `agent wait` と `pane wait-output` は状態や出力が変わったときだけ動きます。`pane wait-output` は待ち始めた後の新しい行を一行ずつ見ます。
- `pane read` と出力では、秘密情報に見える行は隠します。`events` は一行に一つのイベントを書きます（`pane.state`、`attention`、費用つきの `run.completed`）。
- 終了コードは成功 0、エラー 1、タイムアウト 2 です。
- 接続できるのはこのコンピュータの同じユーザーだけです（Mac は権限 0600 のソケット、Windows は現在のユーザーだけが開ける名前付きパイプ）。リクエストごとにデータフォルダーの `control/token` のトークンが要ります。`--profile` で起動したアプリには `mighty --profile <フォルダー> status` で接続します。

### スケジュールとトリガー {#schedules}
{{ui:settings.nav.companion}} タブの {{ui:automation.jobs.sectionTitle}} ボックスで、決めた時刻やイベントでエージェントの作業を始めます。ワークスペースを選ぶと、そのスケジュールとトリガー、次回の実行時刻、前回の結果が表示されます。上部には今日の使用額、1日の予算、今日の自動化のまとめがあります。
- **スケジュール**: 5 項目の cron (`0 9 * * 1-5`) か RRULE の一部 (`FREQ=DAILY|WEEKLY|HOURLY`、`BYHOUR`、`BYMINUTE`、`BYDAY`) で決めます。このコンピューターのタイムゾーンに従い、夏時間で飛ばされた時刻は直後に 1 回、2 回来る時刻は最初の 1 回だけ実行します。
- **トリガー**: GitHub (PR の作成、レビュー依頼、CI 失敗)、Linear (イシューが選んだ状態になる)、Webhook、常時チェック (シェルコマンドを数分ごとに実行し、出力が変わったときだけエージェントを起こす。1 日の上限まで)。GitHub は `gh` でログインしたアカウントで 5 分に 1 回まで確認し、変化がないときや接続が切れたときはもっと間隔を空けます。Linear はワークスペースに接続した Linear を使います。
- **実行方法**: 新しいペイン (必要ならワークツリー) にプロンプトを送るか、{{ui:automation.form.heartbeat}} をオンにすると同じ名前のペインに同じ会話として送ります。エージェントとモデルを選べます。
- **予算**: 今日の使用額にこの実行の見込み費用を足して 1 日の予算を超えるときは、実行せず「スキップ」として記録します。
- **通知**: 自動で始めた作業が承認や回答を待つと、スマートフォンと OS の通知ですぐに知らせます。それ以外は実行履歴と今日のまとめに集めます。すべての実行は `automation.fired`、`automation.skipped`、`automation.blocked`、`automation.finished` イベントとして `mighty events` でも見られ、{{ui:automation.settings.events}} で {{ui:automation.event.automation}} をオンにすると Webhook と状態コマンドも従います。
- {{ui:automation.history.title}}: 実行ごとに時刻、状態、費用 (使用量台帳による)、結果リンク (ペインが開いた PR)、エラーが残ります。{{ui:automation.history.retry}} ボタンで再実行し、{{ui:automation.history.continue}} ボタンでそのペインを開きます。

> [!warning]
> スケジュールとトリガーは **アプリが開いている間だけ** 実行します。アプリを閉じると止まり、[バックグラウンド作業](#background) をオンにしても実行中の作業が続くだけで新しい作業は始まりません。閉じている間に逃したスケジュールは、次にアプリを開いたとき最新の 1 回だけ実行します。{{ui:automation.form.skipMissed}} をオンにすると実行せず「スキップ」として記録します。

**Webhook の受信**: {{ui:automation.listener.toggle}} をオンにすると、このコンピューターの `127.0.0.1:<ポート>` でだけリクエストを受けます (既定はオフ)。Webhook トリガーごとにアドレス (`http://127.0.0.1:<ポート>/hooks/<id>`) とシークレットがあり、シークレットはシステムのキーチェーンに保存されます。リクエスト本文をシークレットで HMAC-SHA256 署名し、`X-Mighty-Signature-256: sha256=<hex>` (GitHub の `X-Hub-Signature-256` も可) ヘッダーで送ってください。256KB を超える本文、署名が違うリクエスト、同じ配信 ID (`X-Mighty-Delivery`) は捨てます。外部 (GitHub や他のサービス) から受けるには、ngrok や Cloudflare Tunnel などのトンネルを自分で前に置いてください。アプリがトンネルを開くことはありません。

リポジトリで共有するスケジュールとトリガーは `.mighty.yaml` に書きます。[他の設定と同じく](delegation.md#mighty-yaml) 承認した後だけ実行され、ボックスでは読み取り専用で表示され、承認前は {{ui:automation.jobs.approve}} ボタンが出ます。

```yaml
schedules:
  deps-check:
    cron: "0 9 * * 1-5"        # または rrule: FREQ=DAILY;BYHOUR=9
    prompt: 依存関係を点検し、必要なら PR を開いてください。
    agent: claude              # codex, gemini, pi, opencode, hermes
    model: sonnet
    worktree: true
    catchUp: once              # once (既定) または skip
  night-watch:
    rrule: FREQ=DAILY;BYHOUR=22
    mode: heartbeat
    pane: 夜間チェック
    prompt: 続けてください。
triggers:
  fix-ci:
    github: [ci_failed]        # pr_opened, review_requested, ci_failed
    prompt: CI の失敗を直してください。
    worktree: true
  ready-issues:
    linear: [Ready]
    prompt: このイシューを始めてください。
  deploy:
    webhook: true
    prompt: デプロイ結果を確認してください。
  flaky-tests:
    standing: npm test 2>&1 | tail -5
    everyMinutes: 30           # 5-1440
    dailyCap: 3                # 1-50
    prompt: テスト結果が変わりました。原因を見つけて直してください。
```

## コンポーネント {#components}
{{ui:settings.nav.tools}}タブでは、アプリに必要なものが準備できているかを確認し、エージェントCLIのインストール、サインイン、アップデートをまとめて管理します。このコンピュータのOS向けの項目だけが表示されます。

一番上の行には、表示の切り替えとボタンが2つあります。
- {{ui:settings.components.filterAll}} / 要対応: 要対応を選ぶと、手を入れる必要がある行だけが表示されます。未インストール、未サインイン、アップデートあり、失敗、対応が必要な項目です。横の数字がその数です。
- 更新(↻、{{ui:setup.button.recheck}}): PATHを読み直し、すべての項目を確認し直します。
- {{ui:settings.components.updateAllButton}}: インストール済みのエージェントCLIを今すぐすべてアップデートします。CLIごとに、最初にインストールした方法(npm、Homebrewなど)をそのまま使います。

一覧は3つのグループです。{{ui:setup.group.agent}}(Claude、Codex、Antigravity、pi、opencode、Hermes)、{{ui:settings.components.groupTools}}(Git(MacではXcodeコマンドラインツール)、Homebrew(Mac)、Node.js、Bun(Mac)、uv、Tailscale、gh、glab)、{{ui:settings.components.groupPlugins}}(ouroboros、omc、gstack(Mac))です。項目ごとに1行で、行には開閉の印(▸)、名前、バージョン、エージェントCLIならサインインしているアカウントがあります。右側には状態と、今いちばん必要なボタンが1つあります。
- 状態: {{ui:setup.status.installed}}、{{ui:settings.components.statusInstalled}}(エージェントCLIがインストールされ、サインインも済み)、{{ui:settings.components.statusUpdateAvailable}}、{{ui:settings.cliAccounts.summarySignedOut}}、{{ui:settings.cliAccounts.statusNotInstalled}}、{{ui:setup.status.attention}}、{{ui:setup.status.installing}}、{{ui:setup.status.failed}}。
- ボタン: ないときは{{ui:settings.components.installButton}}(ダウンロードページからしかインストールできない項目は{{ui:setup.button.openPage}})、サインインしていないときは{{ui:settings.cliAccounts.buttonLogin}}、古いときは{{ui:setup.button.update}}。問題がなければボタンはありません。

行をクリックすると、その場で開きます(▾)。開いた行には詳しい内容(必要なバージョン、前回のアップデートの結果、アカウントの情報、対応が必要な理由、その項目の用途)と、その項目のボタンがすべてあります。エージェントCLIなら{{ui:settings.cliAccounts.buttonChange}}、{{ui:settings.cliAccounts.buttonLogout}}、{{ui:settings.cliAccounts.refreshTooltip}}などです。もう一度クリックすると閉じます。開いた行は、アプリを終了するまで覚えています。

一覧の下の{{ui:settings.components.groupAutoUpdate}}ボックスには、スイッチが3つと前回の実行時刻があります。スイッチにポインタを合わせると説明が表示されます。
- {{ui:settings.components.autoUpdateClis}}: アプリの起動時と、6時間ごとにエージェントCLIのアップデートを確認します。
- {{ui:settings.components.autoUpdatePlugins}}: ClaudeとCodexのプラグインも一緒にアップデートします。
- {{ui:settings.components.autoLogin}}: サインインが切れたら再サインインし、失敗したリクエストを再送信します。

{{ui:settings.components.installButton}}をクリックすると、その項目と、その項目に必要で足りない前提ツールだけをインストールします。実行するコマンドは、先に1つの確認画面ですべて表示されます。
- {{platform:mac}} アプリ内のターミナルペインで実行します。ローカルのワークスペースが開いていないときはターミナル(Terminal.app)で実行します。
- {{platform:windows}} 別のPowerShellウィンドウで実行します。

インストールできたかどうかは、終わったあとに確認し直して判断します。初回起動時に開く[{{ui:setup.title}}](getting-started.md#setup)ウィンドウは、足りないものを選んで{{ui:setup.button.install}}をクリックするチェックリストです。

同じタブの一番下には、{{ui:settings.toolkit.sectionTitle}}ボックスがあります。よく使うツールを自分で集めておく一覧で、最初は空です。{{ui:settings.toolkit.addButton}}でツールを追加し、項目ごとに{{ui:settings.toolkit.approveButton}}と{{ui:settings.toolkit.removeButton}}のボタンがあります。承認したツールだけをインストールでき、インストールの前に実行するコマンドをすべて表示して、{{ui:settings.toolkit.confirmTitle}}を求めます。一覧を{{ui:settings.toolkit.exportButton}}したり{{ui:settings.toolkit.importButton}}したりできます。

![[settings-tools]]

### サインイン {#cli-accounts}
エージェントCLIごとに、その行でサインインします。サインインしていないCLIは行に{{ui:settings.cliAccounts.buttonLogin}}ボタンがそのまま表示され、そのほかのアカウントのボタンは行を開くと表示されます。
1. {{ui:settings.cliAccounts.buttonLogin}}をクリックします。Claudeでは、{{ui:settings.cliAccounts.buttonLoginClaude}}と{{ui:settings.cliAccounts.buttonLoginConsole}}から選びます。
2. CLIのサインインがターミナルで始まります。ブラウザで承認します。
   - {{platform:mac}} アプリ内にターミナルペインが開きます。ローカルのワークスペースに開くため、ワークスペースが1つ以上開いている必要があります。
   - {{platform:windows}} 別のコンソールウィンドウが開きます。ワークスペースがなくてもかまいません。
3. サインインが完了すると行の状態が変わります。変わらないときは、その行を開いて{{ui:settings.cliAccounts.refreshTooltip}}をクリックします。変更したアカウントは次のリクエストから使われます。

アカウントを変更するには行を開いて{{ui:settings.cliAccounts.buttonChange}}、サインアウトするには{{ui:settings.cliAccounts.buttonLogout}}をクリックします。{{ui:settings.components.groupAutoUpdate}}ボックスの{{ui:settings.components.autoLogin}}をオンにすると、サインインが切れたときに自動で再サインインし、失敗したリクエストを再送信します。

Claudeの行を開くと、{{ui:settings.cliAccounts.resetModelsButton}}と{{ui:settings.cliAccounts.bedrockButton}}のボタンもあります。{{ui:settings.cliAccounts.bedrockButton}}はサインインと同じ方法で、ターミナルでClaude CLIのAWS Bedrock設定ウィザードを開きます。終わったら状態を更新してください。{{platform:windows}} Windowsには{{ui:settings.cliAccounts.resetBedrockButton}}ボタンがもう1つあり、Claudeのユーザー設定をバックアップしたうえでBedrockキーとBedrockのモデル指定を削除します。

### アップデート {#cli-update}
エージェントCLIのアップデートは、アプリのアップデートとは別に行います。
- {{ui:settings.components.updateAllButton}}ボタン: インストール済みのエージェントCLIを今すぐすべてアップデートします。古いCLIの行の{{ui:setup.button.update}}ボタンも、CLIのアップデートを実行します。
- {{ui:settings.components.groupAutoUpdate}}ボックスの{{ui:settings.components.autoUpdateClis}}: アプリの起動時と、6時間ごとに確認します。
- {{ui:settings.components.autoUpdatePlugins}}: ClaudeとCodexのプラグインも一緒にアップデートします。

CLIを最初にインストールした方法(npm、Homebrewなど)をそのまま使います。CLIごとの前回のアップデートの結果はそのCLIの行を開くと表示され、プラグインのアップデートの結果は{{ui:settings.components.groupPlugins}}グループの下に表示されます。

## アプリ情報 {#about}
{{ui:settings.nav.about}}タブでは、アプリをアップデートし、データの保存場所を確認します。
- {{ui:settings.appUpdate.sectionTitle}}: 現在のバージョン、自動アップデートのスイッチ、状態とボタン、リリースノートがあります。[アップデート](getting-started.md#update)を参照してください。たたまれた{{ui:settings.advancedTitle}}には{{ui:settings.appUpdate.manifestUrlAddressLabel}}の欄と署名の説明があり、アドレスを入力してあれば最初から開いています。
- {{ui:settings.appInfo.sectionTitle}}: {{ui:settings.appInfo.openHelpButton}}ボタンと{{ui:settings.appInfo.stateLocationLabel}}があります。{{ui:settings.appInfo.stateLocationLabel}}はアプリの設定と履歴があるフォルダで、Macでは{{ui:menu.showInFinder}}、Windowsでは{{ui:menu.showInExplorer}}ボタンで開きます。

![[settings-about]]
