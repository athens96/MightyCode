---
title: リモート接続
order: 10
section: remote
---
Mac または Windows のコンピュータ 2 台の Mighty Code をつなぐと、一方でもう一方のコンピュータのワークスペースを開き、そのコンピュータで Claude・Codex・Antigravity とコマンドを実行できます。ファイルと CLI のログインは実行するコンピュータに置いたままで、プロジェクトのファイルをコピーや同期はしません。

つなぎ方は 2 つあります。
- {{ui:settings.remote.viaPairing}}: スマートフォンのように一度ペアリングし、Mighty リレーを経由して接続します。Tailscale は不要です。[ペアリングで接続](#pair)を参照してください。
- {{ui:settings.remote.viaTailscale}}: 同じ [Tailscale](https://tailscale.com/kb/1017/install) のネットワーク（tailnet）の中で、アドレスと接続キーで直接接続します。このコンピュータのペインをそのコンピュータで実行するリモートペインと大きな添付は、Tailscale でだけ使えます。

相手のコンピューターに Mighty Code を入れられないときは、[SSH マシン](#ssh)としてそのフォルダーで作業できます。

どちらも {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} タブで行います。タブのいちばん上に 1 行の案内があり、その下に {{ui:settings.remote.shareTitle}} | {{ui:settings.remote.connectTitle}} | {{ui:settings.remote.phoneTab}} のタブがあります。内側のタブは、アプリの実行中だけ記憶されます。スマートフォンの接続は[モバイルリモート](phone.md)を参照してください。

## 準備 {#prepare}
- 2 台のコンピュータに Mighty Code をインストールします。
- 作業を任せるコンピュータ（ホスト）に、使う CLI をインストールしてログインします。リモート実行はホストの CLI とアカウントを使います。
- Tailscale で接続するには、2 台のコンピュータに Tailscale をインストールし、同じ tailnet にログインします。ルーターのポート転送や Tailscale Serve・Funnel は不要です。tailnet のポリシーが 2 台の機器の接続を妨げていないことだけ確認してください。

## このコンピュータを共有 {#share}
ホストのコンピュータで:
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.shareTitle}} タブを開きます。
2. {{ui:settings.remote.pickWorkspaces}} で、共有するローカルワークスペースにチェックを入れます。
3. 下の {{ui:settings.remote.methodsTitle}} で使う方式を準備します（下の説明）。
4. 上部の状態行の横にある {{ui:settings.remote.startSharing}} を押します。共有中はその場所に {{ui:settings.remote.sharedListTitle}} の一覧が表示されます。

{{ui:settings.remote.methodsTitle}} には方式ごとに 1 行あります。
- {{ui:settings.remote.viaPairing}}: {{ui:settings.remote.pairAccept}} スイッチとリレーの状態があり、スイッチをオンにすると {{ui:settings.remote.pairCopyLink}} ボタンが表示されます。リンクにはコンピュータ専用のペアリングコード（スマートフォンの QR コードとは別）が含まれるため、秘密としてコピーされ、1 分後にクリップボードから消去されます。リレーを使えない実行では、スイッチは淡色表示になり理由が表示されます。[ペアリングで接続](#pair)を参照してください。
- {{ui:settings.remote.viaTailscale}}: このコンピュータに Tailscale のアドレスがないときは、その旨の説明と {{ui:settings.remote.tailscaleInstall}} ボタンが表示され、押すと {{ui:settings.nav.tools}} タブが開きます。共有の前は {{ui:settings.remote.portLabel}} の欄があります。通常はそのまま（43137）にします。共有中は、このコンピュータの Tailscale アドレスと {{ui:settings.remote.keyLabel}} が表示されます。キーは表示ボタンで確認し、{{ui:settings.remote.copyButton}} でコピーして接続する側に伝えます。

Tailscale がなくても、{{ui:settings.remote.pairAccept}} がオンなら共有を開始できます。このときはペアリングしたコンピュータだけがリレーで接続します。

共有中は {{ui:settings.remote.allowFullAccess}} スイッチが表示されます。オフのあいだ（既定）は、接続したコンピュータはリモートコマンドのペインを使えず、エージェントをフルアクセスで実行することもできません。そうしたリクエストは実行前に断られ、そのペインに理由が表示されます。共有を開始するたびにオフに戻ります。

このスイッチがオフのあいだ、接続したコンピュータは、共有しているコンピュータが共有フォルダの中に閉じ込められないエージェントのペインも使えません（権限モードにかかわらず）。Mac で共有しているときは Antigravity・pi・opencode・Hermes（どれも閉じ込めるサンドボックスがありません）、Windows で共有しているときは Codex を含む Claude 以外のすべてのエージェントです（Windows は Codex を Mac のようにサンドボックスに閉じ込められません）。Mac の Codex は自前のサンドボックスの中で動き、Claude のペインはどちらでもオフのまま実行され、いつもどおり許可を求めます。こうしたペインでは送信だけでなく、ツールの許可、質問への回答、計画の承認にもこのスイッチが必要です。

Tailscale の共有はこのコンピュータの Tailscale アドレスでだけ接続を受け付けます。アプリを開き直すと共有は必ずオフで始まります。{{ui:settings.remote.stopSharing}} を押すとリモート実行が止まり、接続キーは破棄されます。もう一度共有するときは新しいキーを伝えてください。

> [!warning]
> 接続した機器は、共有したワークスペースでエージェントとコマンドを実行できます。権限モードによっては、ホストのユーザー権限でフォルダの外にもアクセスできます。Tailscale の接続キーを持つ機器はすぐに接続できるため、キーは信頼できる機器にだけ教えてください。ペアリングリンクは一度だけ使え、ホストで {{ui:settings.mobileRemote.approval.allow}} を押さないと接続されません。

## 別のコンピュータに接続 {#connect}
作業を頼むコンピュータで:
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.connectTitle}} タブを開きます。{{ui:workspace.addPane}} メニューの {{ui:remote.menu.connect}} からも同じ画面が開きます。
2. 上の 2 択のスイッチで {{ui:settings.remote.pairLinkField}} か Tailscale のアドレスを選びます。
   - {{ui:settings.remote.pairLinkField}}: ホストから受け取ったリンクを貼り付け、必要なら {{ui:settings.remote.nameField}} を付けます。
   - Tailscale のアドレス: {{ui:settings.remote.addressField}}（Tailscale の IP または MagicDNS 名）と {{ui:settings.remote.keyField}} を入れ、必要なら名前を付けます。ホストがポートを変えていなければ、アドレスだけで足ります。
3. {{ui:settings.remote.connectButton}} を押します。

下の {{ui:settings.remote.connectionsTitle}} 一覧には接続ごとに 1 行あります。行にはコンピュータ名、方式のチップ（ペアリング・Tailscale）、状態（{{ui:settings.remote.status.connected}}、{{ui:settings.remote.status.disconnected}}、{{ui:settings.remote.status.saved}}）と {{ui:settings.remote.refreshButton}}、{{ui:settings.remote.disconnectButton}} ボタン、削除アイコンがあります。接続したコンピュータの下には共有中のワークスペースが表示されます。接続すると、そのコンピュータの共有ワークスペースが開いているペインごと自動でサイドバーに現れ、{{ui:remote.mirror.view}} でそのワークスペースへ直接移動できます（[そのコンピュータのペインをそのまま](#mirror)）。タブのいちばん下に、たたまれた {{ui:settings.remote.scopeTitle}} があります。

リモートワークスペースはサイドバーでコンピュータのアイコンとホスト名で見分けられ、ワークスペースの見出しと下のステータスバーに接続状態が表示されます。アプリを開き直しても接続は保存されたまま残り、リモートペインで最初に送信したときか {{ui:settings.remote.refreshButton}} を押したときに再接続します。

{{platform:mac}} 接続キーは macOS のキーチェーンにだけ保存し、ワークスペースの記録には入れません。再接続のときに macOS がキーチェーンへのアクセスを尋ねたら「常に許可」を選んでください。アプリは同じ署名で配布されるので、この選択はアップデート後も保たれます。

{{platform:windows}} Windows でも接続キーをワークスペースの記録には入れず、この Windows ユーザーだけが読めるよう暗号化してアプリのデータフォルダに保存します。Tailscale で共有するには、ファイアウォールの規則が 1 つ必要です（[Windows ファイアウォール](#firewall)）。

## ペアリングで接続（Tailscale なし） {#pair}
どちらのコンピュータにも Tailscale がなくても、スマートフォンのように一度ペアリングすれば、[モバイルリモート](phone.md)と同じ Mighty リレーを経由して接続できます。リレーはアプリに組み込まれているので、別に用意する必要はありません。接続はエンドツーエンドで暗号化され、リレーは内容を読めません。接続した側は[そのコンピュータのペインをそのまま](#mirror)使います。

ホストのコンピュータで:
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.shareTitle}} タブの {{ui:settings.remote.methodsTitle}} で、{{ui:settings.remote.viaPairing}} の行の {{ui:settings.remote.pairAccept}} をオンにします。
2. ワークスペースを選び、{{ui:settings.remote.startSharing}} を押します。共有中はこのコンピュータがリレーに接続し、その状態が同じ行に表示されます。
3. {{ui:settings.remote.pairCopyLink}} でリンクをコピーし、接続するコンピュータに渡します。このリンクはコンピュータ専用で（スマートフォンのQRコードではコンピュータを接続できず、その逆もできません）、一度だけ使え、この画面を閉じると10分後に期限切れになります。相手のコンピュータが接続しようとすると、このコンピュータに「新しいデバイスの接続要求」ウインドウと通知が表示されるので、相手が正しければ {{ui:settings.mobileRemote.approval.allow}} を押します（60秒以内に応答がないと拒否されます）。

接続するコンピュータで:
1. {{ui:settings.remote.navTitle}} → {{ui:settings.remote.connectTitle}} タブのスイッチで {{ui:settings.remote.pairLinkField}} を選び、リンクを貼り付け、必要なら名前を付けて {{ui:settings.remote.connectButton}} を押します。ホストで許可されるまで、そちらで許可するよう案内が表示されます。
2. 接続すると、そのコンピュータが {{ui:settings.remote.connectionsTitle}} 一覧にペアリングのチップ付きで、サイドバーに「リモート · 名前」として現れます。接続と切断は Tailscale の接続と同じです。

- リンクにはコンピュータ専用の、短時間だけ使えるペアリングコードが含まれます。リンクが他人に渡っても、ホストで許可しなければ接続されません。最初の接続でホストがこのコンピュータ専用のデバイストークンを渡し、以後はそのトークンで接続するので、改めて許可を求められません。トークンは接続キーと同じく macOS のキーチェーン（Windows ではこのユーザーだけが復号できる暗号化ファイル）に保存し、ワークスペースの記録には入れません。ホストで {{ui:settings.remote.pairAccept}} がオフのとき、または共有していないときは、新しいコンピュータは接続できません。
- ペアリングしたコンピュータはホストの {{ui:settings.remote.phoneTab}} タブの {{ui:settings.mobileRemote.connectedDevicesTitle}} 一覧にもコンピュータのアイコン付きで表示されます（スマートフォン接続のスイッチがオフでも、再起動後も表示されます）。その行の {{ui:settings.mobileRemote.revokeRowButton}} ボタンはそのコンピュータだけを解除し、ほかのスマートフォンとコンピュータは接続されたままです。{{ui:settings.mobileRemote.regenerateKeyButton}} を押すとスマートフォンとコンピュータがすべて解除され、ホストのリレーIDと暗号キーが作り直されるため、再び使うには新しいリンクで接続し直します。
- 見えるのは共有したワークスペースだけで、{{ui:settings.remote.allowFullAccess}} スイッチと閉じ込められないエージェントの規則も Tailscale の接続とまったく同じに適用されます。
- スマートフォンの設定は変わりません。{{ui:settings.remote.phoneTab}} タブでスマートフォンからの接続を許可していなければ、このスイッチがオンでもスマートフォンは接続できません。
- リレーは一度に 1 MB まで送ります。ペアリング接続で送る添付ファイルは合計約 750 KB までで（ファイルの内容は base64 で大きくなります）、超えると送る前に知らせます。大きなファイルは Tailscale で接続して送ってください。長い会話は 1 MB に収まる新しい項目から表示されます。
- ペアリング接続で使えるのは[そのコンピュータのペインをそのまま](#mirror)だけです。このコンピュータのペインをそこで実行するリモートペイン（{{ui:settings.remote.openButton}} ボタン）には Tailscale の接続が必要です。
- ホストが共有を止めるかスイッチをオフにすると、接続した側に共有が止まったと表示されます。ホストが再び共有したら、新しいリンクなしで {{ui:settings.remote.refreshButton}} を押して再接続できます。

## Windows ファイアウォール {#firewall}
{{platform:windows}} Windows では {{ui:settings.remote.shareTitle}} タブと {{ui:settings.remote.phoneTab}} タブの下に {{ui:settings.remote.firewallTitle}} があります。Tailscale の範囲からの接続にだけアプリのポートを開く規則を 1 つ管理し、今の状態が 1 行で表示されます。
- {{ui:remote.windows.firewall.add}} ボタンで規則を追加します。このとき Windows の管理者の確認が 1 回表示されます。
- 規則がすでにあるときは、同じ場所に {{ui:remote.windows.firewall.update}} ボタンが出て、今使っているポートに合わせて直します。規則が今使っているポートをすべて開いていないときは、更新が必要だと表示します。
- {{ui:remote.windows.firewall.remove}} ボタンで規則を削除します。

この 1 つの規則が、ワークスペースの共有のポートとスマートフォンの[直接接続](phone.md#direct)のポートを両方開きます。そのため、どちらのタブで変更しても同じ規則が変わり、規則を削除すると両方のポートが閉じます。ペアリング接続はリレーを経由するので、この規則は不要です。

## そのコンピュータのペインをそのまま {#mirror}
別のコンピュータに接続すると、そのコンピュータが共有しているワークスペースが、そこで開いているエージェントペインごとサイドバーに現れます。会話、{{ui:graph.view.mighty}} ビュー、進捗と状態がリアルタイムで反映され、ここから送信・停止や、承認・質問・計画への回答ができます。ペインはそのコンピュータで、その CLI とアカウントで動きます。送った内容はそのペインの次のリクエストになり、実行中ならそのコンピュータのキューに入ります。

- サイドバーには、このコンピュータのワークスペースの下にサーバーごとの「リモート · サーバー名」見出しができ、アドレスと接続状態が表示されます。緑の点は接続中、オレンジは再接続中、グレーは切断です。見出しの … メニューに {{ui:remote.mirror.connect}}・{{ui:remote.mirror.disconnect}} があります。
- リモートペインの上には {{ui:remote.mirror.paneBadge}} の帯が付き、どのコンピュータで動いているか、接続状態、そのコンピュータのキューにあるリクエスト数を示します。下のステータスバーも「リモート · そのコンピュータで実行中 · 接続中」のように表示します。
- {{ui:workspace.addPane}} メニューでそのコンピュータに新しいエージェントペインを開けます。ペインを閉じるとそのコンピュータのペインも閉じます。
- 切断するとペインは最後の状態のまま残り、送信できません。接続を削除するとそのサーバーのワークスペースが消えます。これらのペインはこのコンピュータに保存されないため、アプリを開き直したら接続すると再び表示されます。
- 会話は最新 80 件まで表示され、画像は表示されません。ペイン名の変更やモデル・権限モードの変更はそのコンピュータで行います。コマンドペインは表示されず、[リモートペインで使えない機能](#use)も同じです。
- フルアクセスで動くペインへの送信と「常に許可」は、そのコンピュータが {{ui:settings.remote.allowFullAccess}} をオンにしているときだけ使えます。 そのコンピュータが閉じ込められないエージェントのペインへの送信と回答（ツールの許可、質問、計画）にもこのスイッチが必要です。Mac なら Antigravity・pi・opencode・Hermes、Windows なら Claude 以外のすべてのエージェントです。
- そのコンピュータの Mighty Code がこの機能のない古いバージョンなら、ペインは表示されず見出しに理由が出ます。その場合は {{ui:settings.remote.connectionsTitle}} 一覧の {{ui:settings.remote.openButton}} で、そのワークスペースをこのコンピュータのリモートペインとして使います（Tailscale の接続）。

## リモートペインを使う {#use}
- エージェントペインはローカルのペインと同じように使えます。モデル・権限モード・思考の強さ・Fast モードはそのままホストに伝わり、ホストの CLI が対応していない設定は実行前に知らせます。
- 権限・質問・計画のカードはこのアプリに表示され、答えはホストの実行に届きます。実行中に送ったリクエストは、ローカルのペインと同じようにキューに入り、今の実行が終わってから始まります。
- {{ui:graph.view.mighty}} 表示（{{ui:graph.view.diagram}}・{{ui:graph.view.timeline}}・{{ui:graph.view.structure}}）は、ローカルの実行とまったく同じように描かれます。
- {{ui:remote.menu.newCommand}} のペインは、コマンドごとにホストの共有フォルダで新しいシェルを開きます。対話型プログラムとパスワード入力には対応していません。
- リモートペインは、ステータスバーとセッション情報に実行コンピュータの OS（macOS・Windows）を表示します。{{ui:remote.menu.newCommand}} のペインは、Mac のホストではログインシェル、Windows のホストでは PowerShell でコマンドを実行します。
- 添付したファイルは内容そのものがホストに送られます。一度に 8 個、1 ファイル 5 MiB、合計 8 MiB までです。

リモートワークスペースでは次のものを使えません。どれもこのコンピュータのディスクやネットワークを読むためです。
- [Mightyオーケストレーション](delegation.md)（子のワークツリー）
- ファイルペイン
- プロジェクトのファイルを読む[スタイル](styles.md)、ステータスライン、以前のセッションの再開

pi、opencode、Hermesのペインはまだリモートワークスペースでは開けません。リモートワークスペースは[スマートフォンのリモート](phone.md)には表示されません。

## 接続が切れたとき {#disconnect}
- ネットワークが切れると、実行中のリモートペインはエラーで終わり、{{ui:settings.remote.connectionsTitle}} 一覧に理由が表示されます。ホストは応答が途絶えた実行を約 30 秒以内に自分で止めます。
- {{ui:settings.remote.disconnectButton}} はその接続の実行を止め、接続情報は残します。もう一度使うには {{ui:settings.remote.refreshButton}} を押すか、リモートペインから送信します。接続情報まで消すには、その行の削除アイコンを押します。
- Tailscale のホストが共有をやり直した場合は新しいキーが必要です。同じアドレスに新しいキーで接続すると、保存された接続のキーが置き換わります。
- ホストのコンピュータがスリープ中か、そこで Mighty Code が終了しているあいだは、新しい作業を送れません。

## SSH マシン {#ssh}
相手のコンピューターに何もインストールせず、ペアリングもせずに、SSH でそのフォルダーのターミナルとエージェントを動かします。開発サーバー、クラウド VM、Linux 専用ツールのあるマシンに向いています。

**準備**
- このコンピューターに OpenSSH クライアントが必要です。Mac には標準で入っています。Windows は「OpenSSH クライアント」機能をオンにします。
- ログインはこのコンピューターの OpenSSH が行います。`~/.ssh/config`、ssh-agent、鍵をそのまま使い、アプリはパスワードや鍵を保存しません。パスワードなしで `ssh ホスト` に入れるようにしておいてください。
- 初めて使うマシンは、ターミナルで `ssh ホスト` を一度実行してホスト鍵を確認し、受け入れておきます。アプリがホスト鍵を代わりに受け入れることはありません。

**マシンを追加する**
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} タブの {{ui:ssh.settings.sectionTitle}} ボックスを開きます。
2. {{ui:ssh.settings.fromConfig}} メニューで `~/.ssh/config` の Host を選ぶか、`user@host:ポート` を入力して {{ui:ssh.settings.addButton}} を押します。ワイルドカードの Host と `Match` ブロックは一覧に出ません。
3. {{ui:ssh.settings.test}} はパスワードを尋ねずに接続し、OS、ログインシェル、tmux の有無を確認します。「接続できない」「ログイン拒否」「未知のホスト鍵」を区別して知らせます。
4. リモートフォルダー(`~/src/app` や `/srv/app` のように `~` か `/` で始まる)を入力して {{ui:ssh.settings.open}} を押すと、そのフォルダーがワークスペースとして開き、ターミナルペインが 1 つできます。

マシンはサイドバーに「SSH · 名前」の見出しで表示され、その下にワークスペースが並びます。見出しの … メニューから {{ui:ssh.menu.test}} ができます。

**できること**
- ターミナルペイン: そのマシンのログインシェルがそのフォルダーで開きます。
- エージェントペイン: Claude、Codex、Antigravity、pi は、そのマシンにインストールされた CLI がそのマシンのアカウントで動きます。承認や質問のカードはこのアプリに表示されます。CLI がそのマシンにないと実行はエラーで終わります。
- そのマシンのログインシェルは bash や zsh のほか、fish・csh・tcsh でも使えます。
- ファイルペイン: そのマシンのファイルを開いて編集し、保存できます(1 ファイル 1 MiB まで)。保存はそのマシン上で一度に置き換わり、エンコーディング・改行・権限はそのまま保ちます。開いた後にエージェントや他の人がそのファイルを変えていたら上書きせずに知らせ、再読み込みか上書きを選べます。
- Mac では同じマシンのペインが 1 本の SSH 接続を共有します。Windows の OpenSSH はこれに対応していないため、ペインごとに接続します。
- ローカルに接続: ペインの出力に `localhost:3000` のような開発サーバーのアドレスが出ると、ペイン上の SSH バーに接続ボタンが出ます。このコンピューターの空きポートに転送し、アドレスを押すとブラウザーで開きます。ペインを閉じると転送も終わります。
- [バックグラウンド作業](settings.md#general)をオンにしておくと、アプリを閉じても SSH ペインの作業は続きます。

**接続が切れたら**
- ターミナルペインに「SSH 接続が切れました。」と {{ui:ssh.terminal.reconnect}} ボタンが表示されます。
- マシンに tmux があれば {{ui:ssh.settings.tmux}} スイッチがオンになり、ターミナルペインは tmux セッション内で動きます。接続が切れても作業はマシン上で続き、再接続すると同じセッションに戻ります。スイッチはマシンごとにオフにできます。
- エージェントペインの実行はそのマシンで続きます。このアプリは 2・5・15・30・60・120 秒の間隔(その後は 120 秒ごと)で再接続し、切れたところから読み続けます。待っていた承認カードもそのまま残り、切断中に答えた内容は再接続後に CLI に届きます。30 分以内に戻らないと、このアプリではエラーで終わり、そのマシンでは終わるまで続く場合があります。

**まだできないこと**
- リクエストの変更レビュー(変更部分ごとの受け入れ・破棄)、リモートの git 変更表示、作業マップ、ワークツリー、マイティオーケストレーション、添付、opencode と Hermes のペイン。

**Mighty Code サーバー(リモートの Mac)**
マシンが Mac なら、Mighty Code をウィンドウなしで動かしておき、その Mac のワークスペースとエージェントペインをこのコンピューターで[そのまま](#mirror)使えます。Tailscale やペアリングのリンクは使わず、このマシンの SSH だけを使います。Linux・Windows のマシンにはインストールできません(Linux 用のビルドはありません)。上のインストールなしの方式で使います。

1. マシンの下の {{ui:ssh.server.title}} で {{ui:ssh.server.detect}} ボタンを押すと、macOS のバージョン、インストール済みのアプリ(`~/Applications`、なければ `/Applications`)とそのバージョン、サーバーが動いているかを確認します。
2. {{ui:ssh.server.install}} ボタン: このアプリがアップデート情報からこのアプリと同じバージョンの Mac 用パッケージをダウンロードし(そのバージョンがなければ最新版を使い、確認画面で知らせます)、アップデート署名を確認してから SSH でその Mac の `~/Applications/MightyCode.app` にインストールし、ウィンドウなしで起動します。その Mac でもコード署名を確認し、管理者権限(sudo)は使いません。
3. インストールが終わるとすぐ接続します。サイドバーに「リモート · 名前」が現れ、その Mac のワークスペースが表示されます。あとで {{ui:ssh.server.connect}} ボタンやサイドバーの接続で再接続できます。
- その Mac のサーバーのバージョンがこのアプリと違うと {{ui:ssh.server.update}} ボタンが表示されます。実行中のサーバーを停止し、新しいバージョンに置き換えて再起動します。
- アプリはその Mac の画面に同じユーザーがログインしているときだけ起動します。モニターのない Mac では、その Mac で自動ログインをオンにしておいてください。
- SSH 接続が切れてもサーバーはその Mac で動き続け、このアプリが何度か再接続します。SSH はつながったままその Mac のサーバーだけが再起動したときも、このアプリが自動でペアリングし直してつなぎます。
- {{ui:ssh.server.remove}} ボタンはサーバーを停止して `~/Applications/MightyCode.app` を削除します(先に確認します)。その Mac の Mighty Code のデータは残り、`/Applications` のアプリには触れません。
