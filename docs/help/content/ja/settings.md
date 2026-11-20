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
- その下にOSの通知の許可状態が表示され、{{ui:windows.notifications.settingsButton}}ボタンでシステムの通知設定を開きます。

![[settings-companion]]

![[companion-pet]]

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
