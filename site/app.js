(function () {
  const PUBLIC_BASE = "https://pub-b705901d18c940159d40f58683d758ad.r2.dev/mightycode";

  const I18N = {
    ko: {
      "meta.title.main": "Mighty Code",
      "meta.title.about": "About — Mighty Code",
      "nav.main": "메인",
      "nav.docs": "문서",
      "nav.about": "About",
      "nav.download": "다운로드",
      "home.hero.title": "여섯 에이전트를,<br>한 창에서.",
      "home.hero.lead": "Claude Code, Codex, Antigravity, pi, opencode, Hermes를 나란히 띄우고, 하는 일을 블록 흐름으로 지켜보세요.",
      "home.hero.how": "어떻게 동작하나요 ›",
      "home.hero.alt": "Mighty Code 화면: 사이드바와 에이전트·터미널 창",
      "home.agents.title": "이미 쓰는 에이전트 CLI 그대로",
      "home.statement": "터미널 로그를 읽는 대신,<br>에이전트가 지금 무엇을 하는지<br>한눈에 보세요.",
      "home.more": "자세히 보기 ›",
      "home.st.running": "실행 중",
      "home.st.done": "완료",
      "home.st.writing": "작성 중…",
      "home.flow.kicker": "마이티 보기",
      "home.flow.title": "요청부터 결과까지,<br>흐름으로.",
      "home.flow.s1.t": "요청을 보냅니다.",
      "home.flow.s1.d": "실행 중에도 다음 요청을 대기열에 쌓아 둘 수 있습니다.",
      "home.flow.s2.t": "단계가 블록으로 자랍니다.",
      "home.flow.s2.d": "하위 에이전트와 백그라운드 명령이 나타나는 대로 이어집니다.",
      "home.flow.s3.t": "결과로 화면이 이동합니다.",
      "home.flow.s3.d": "바꾼 줄과 함께 최종 결과가 도착하면 그 블록으로 부드럽게 옮겨 갑니다.",
      "home.flow.req": "요청 7 · Claude",
      "home.flow.reqText": "로그인 화면에 “비밀번호 찾기” 링크를 넣어 줘",
      "home.flow.step1": "1. 화면 코드 찾기",
      "home.flow.step2": "2. 하위 에이전트 · 테스트",
      "home.flow.result": "결과",
      "home.flow.resultText": "링크를 추가하고 테스트 4개가 모두 통과했습니다.",
      "home.orch.kicker": "마이티 오케스트레이션",
      "home.orch.title": "기다리지 말고,<br>나눠 맡기세요.",
      "home.orch.s1.t": "⌘Enter로 옆 창에 보냅니다.",
      "home.orch.s1.d": "실행 중이면 대기열로 보낼지, 어느 에이전트의 워크트리로 보낼지 고릅니다.",
      "home.orch.s2.t": "자식은 자기 브랜치에서 일합니다.",
      "home.orch.s2.d": "저마다 git worktree <code>mighty/&lt;id&gt;</code>를 가져 서로 부딪히지 않습니다.",
      "home.orch.s3.t": "검토하고 병합합니다.",
      "home.orch.s3.d": "바뀐 내용을 확인한 뒤 부모 브랜치에 합치거나 버립니다.",
      "home.orch.parent": "부모 · Claude",
      "home.orch.merged": "병합 완료 · 3",
      "home.num.agents": "나란히 쓰는 에이전트 CLI",
      "home.num.unit": "개",
      "home.num.queue": "창마다 쌓아 두는 다음 요청",
      "home.num.e2e": "휴대폰과 주고받는 모든 내용의 종단 간 암호화",
      "home.phone.kicker": "휴대폰 원격",
      "home.phone.title": "자리를 비워도,<br>일은 멈추지 않게.",
      "home.phone.desc": "승인이 필요하면 휴대폰에 알림이 옵니다. 내용을 보고 그 자리에서 답하세요. 새 기기는 컴퓨터에서 직접 허락해야만 연결됩니다.",
      "home.phone.lock": "릴레이는 암호문만 전달합니다 · X25519 · ChaCha20-Poly1305",
      "home.phone.notifTitle": "Mighty Code · 로그인 수정",
      "home.phone.notif": "Claude가 명령 실행 승인을 기다립니다",
      "home.phone.r1": "로그인 수정",
      "home.phone.r1s": "대기",
      "home.phone.r1d": "승인을 기다리는 중",
      "home.phone.r2": "API 정리",
      "home.phone.r2s": "실행 중",
      "home.phone.r2d": "2. 하위 에이전트 · 테스트",
      "home.phone.r3": "문서 사이트",
      "home.phone.r3s": "완료",
      "home.phone.r3d": "가이드를 고쳤습니다",
      "home.phone.ask": "명령을 실행할까요?",
      "home.phone.askFrom": "Claude · 로그인 수정",
      "home.phone.deny": "거절",
      "home.phone.allow": "승인",
      "home.bento.title": "작은 것까지, 다 챙겼습니다.",
      "home.plan.t": "스타일과 계획 카드",
      "home.plan.d": "오로보로스나 클러드 플랜으로 정해진 순서대로. 계획을 읽고 승인하거나 고쳐 달라고 하세요.",
      "home.pet.t": "Mighty Pet",
      "home.pet.d": "진행 상황과 승인 요청을 화면 구석에서.",
      "home.queue.t": "요청 대기열",
      "home.queue.d": "실행 중에도 계속 말을 거세요.",
      "home.queue.q1": "다크 모드도 맞춰 줘",
      "home.queue.q2": "테스트 하나 더",
      "home.queue.q3": "README 갱신",
      "home.search.t": "대화 검색",
      "home.search.d": "지난 대화 속 그 한 줄을 바로.",
      "home.search.q": "비밀번호 찾기 링크",
      "home.dash.t": "작업 현황",
      "home.dash.d": "모든 워크스페이스의 창을 상태 순으로, 계정 사용량까지.",
      "home.term.t": "터미널 · 파일",
      "home.term.d": "Mac은 Ghostty, Windows는 PowerShell. 소스·Markdown·이미지 미리 보기와 변경점 비교까지.",
      "home.remote.t": "다른 컴퓨터 열기",
      "home.remote.d": "페어링 코드로 다른 Mac·Windows의 워크스페이스를 그대로 엽니다. 처음 연결은 상대 컴퓨터가 허락해야 합니다.",
      "home.remote.here": "이 Mac",
      "home.remote.there": "작업실 PC",
      "home.review.t": "검토 대기열",
      "home.review.d": "에이전트가 바꾼 파일을 차례로 살펴보고, 원하는 편집기에서 바로 엽니다.",
      "home.review.open": "편집기에서 열기",
      "home.final.title": "지금 시작하세요.",
      "hero.meta": "macOS · Windows · Android",
      "dl.title": "다운로드와 설치",
      "dl.versionPrefix": "버전 ",
      "dl.sub": "내 운영체제용 파일을 받습니다. 설치 후 새 버전은 앱 안에서 서명을 확인한 뒤 자동으로 받아 준비합니다.",
      "dl.mac.desc": "macOS 14 이상. 열고 Mighty Code를 응용 프로그램 폴더로 끌어다 놓습니다.",
      "dl.mac.dmg": "Mac 앱 (MightyCode-macos.dmg)",
      "dl.mac.zip": "Mac 앱 (MightyCode-macos.zip)",
      "dl.mac.badge": "Apple 공증 완료 · 보안 경고 없이 열립니다",
      "dl.win.tag": "베타 · x64 / ARM64",
      "dl.win.desc": "Windows 10(버전 2004) 이상 또는 Windows 11. 관리자 권한 없이 이 사용자에게만 설치됩니다.",
      "dl.win.x64": "Windows 설치 프로그램 (x64)",
      "dl.win.arm64": "Windows 설치 프로그램 (ARM64)",
      "dl.and.desc": "휴대폰에 받아 설치하고 컴퓨터의 설정 → 모바일 리모트에서 QR 코드로 연결합니다.",
      "dl.and.apk": "Android 앱 (MightyCode-android.apk)",
      "dl.and.guide": "휴대폰 원격 도움말 →",
      "about.overview.title": "개요",
      "about.overview.desc": "<b>Mighty Code</b>는 프로젝트 폴더(워크스페이스)마다 여러 실행 창을 열고, 창마다 에이전트 CLI(Claude Code, Codex, Antigravity, pi, opencode, Hermes)를 직접 실행해 대화와 설정을 따로 유지하는 데스크톱 앱입니다. 화면은 한국어·영어·중국어·일본어로 볼 수 있습니다.",
      "about.arch.title": "구조 (Rust + Tauri 2)",
      "about.arch.desc": "macOS 앱과 Windows 앱은 하나의 Rust + Tauri 2 워크스페이스(<code>native/tauri</code>)로 만들어집니다. 공통 로직(<code>mighty-core</code>, <code>mighty-run</code>, <code>mighty-graph</code>, <code>mighty-delegation</code>, <code>mighty-remote</code>, <code>mighty-tools</code>)은 OS 코드 없이 공유하고, 플랫폼마다 다른 부분은 포트(<code>mighty-platform</code>)에 각 운영체제 구현을 끼워 넣습니다.",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code>(AppKit). 터미널은 네이티브 <b>Ghostty</b>(libghostty C API, Metal) 뷰를 올리고, 웹 페이지는 기본 브라우저에서 엽니다. macOS 14 이상, Apple Silicon.",
      "about.arch.win": "Rust + Tauri 2 + WebView2. 터미널은 Rust <b>ConPTY + xterm.js</b>를 쓰고, 프로세스 관리는 Job Object, 비밀값 저장은 DPAPI를 씁니다. Windows 10(2004) 이상 또는 Windows 11, x64·ARM64.",
      "about.arch.mob": "React Native(Expo) Android 앱. 릴레이 서버를 거쳐 종단 간 암호화(X25519 → HKDF → ChaCha20-Poly1305)로 연결하며, 같은 Tailscale 네트워크면 컴퓨터에 직접 연결합니다.",
      "about.sec.title": "동작 원칙과 지원 범위",
      "about.sec.list": "<li>앱은 <code>sudo</code>를 쓰지 않고 시스템 비밀번호를 받지 않습니다.</li><li>릴레이는 암호문만 전달하고 아무 비밀도 모르며, 파일로 된 스타일은 내용을 확인해 허용한 뒤에만 씁니다.</li><li>자동 업데이트는 매니페스트의 Ed25519 서명을 확인한 뒤에만 설치합니다.</li><li>Claude를 뺀 에이전트 창(Codex, Antigravity, pi, opencode, Hermes)에는 베타 표시가 붙습니다.</li><li><code>pi</code>, <code>opencode</code>, <code>Hermes</code> 창은 아직 마이티 오케스트레이션과 원격 워크스페이스에서 쓸 수 없습니다.</li>",
      "about.oss.title": "저장소와 빌드",
      "about.oss.desc": "소스 코드, 릴레이(Cloudflare Workers / Oracle Cloud 배포 스크립트), 도움말 원문, CI 워크플로는 GitHub 저장소에 있습니다.",
      "about.oss.mac": "macOS 빌드 ↗",
      "about.oss.win": "Windows 빌드 ↗"
    },
    en: {
      "meta.title.main": "Mighty Code",
      "meta.title.about": "About — Mighty Code",
      "nav.main": "Home",
      "nav.docs": "Docs",
      "nav.about": "About",
      "nav.download": "Download",
      "home.hero.title": "Six agents.<br>One window.",
      "home.hero.lead": "Run Claude Code, Codex, Antigravity, pi, opencode and Hermes side by side, and watch what they do as a flow of blocks.",
      "home.hero.how": "See how it works ›",
      "home.hero.alt": "Mighty Code: the sidebar with agent and terminal panes",
      "home.agents.title": "The agent CLIs you already use",
      "home.statement": "Stop reading terminal logs.<br>See what every agent is doing<br>at a glance.",
      "home.more": "Learn more ›",
      "home.st.running": "Running",
      "home.st.done": "Done",
      "home.st.writing": "Writing…",
      "home.flow.kicker": "Mighty View",
      "home.flow.title": "From request to result,<br>as a flow.",
      "home.flow.s1.t": "Send a request.",
      "home.flow.s1.d": "Queue up the next ones while the agent is still working.",
      "home.flow.s2.t": "Steps grow as blocks.",
      "home.flow.s2.d": "Sub-agents and background commands join the flow as they appear.",
      "home.flow.s3.t": "The view glides to the result.",
      "home.flow.s3.d": "When the final answer arrives with its changed lines, the diagram moves to it.",
      "home.flow.req": "Request 7 · Claude",
      "home.flow.reqText": "Add a “Forgot password?” link to the sign-in screen",
      "home.flow.step1": "1. Find the screen code",
      "home.flow.step2": "2. Sub-agent · tests",
      "home.flow.result": "Result",
      "home.flow.resultText": "Added the link; all 4 tests pass.",
      "home.orch.kicker": "Mighty Orchestration",
      "home.orch.title": "Don’t wait.<br>Hand it off.",
      "home.orch.s1.t": "Send it beside with ⌘Enter.",
      "home.orch.s1.d": "While the agent runs, choose the queue or an agent’s own worktree.",
      "home.orch.s2.t": "Each child works on its own branch.",
      "home.orch.s2.d": "Every child gets a git worktree <code>mighty/&lt;id&gt;</code>, so nothing collides.",
      "home.orch.s3.t": "Review, then merge.",
      "home.orch.s3.d": "Check the changes, then merge them into the parent branch or discard them.",
      "home.orch.parent": "Parent · Claude",
      "home.orch.merged": "Merged · 3",
      "home.num.agents": "agent CLIs side by side",
      "home.num.unit": "",
      "home.num.queue": "requests queued per pane",
      "home.num.e2e": "end-to-end encryption for everything sent to your phone",
      "home.phone.kicker": "Phone Remote",
      "home.phone.title": "Step away.<br>The work keeps going.",
      "home.phone.desc": "When an agent needs approval, your phone gets a notification. Read it and answer right there. A new device connects only after you allow it on the computer.",
      "home.phone.lock": "The relay only carries ciphertext · X25519 · ChaCha20-Poly1305",
      "home.phone.notifTitle": "Mighty Code · Sign-in fix",
      "home.phone.notif": "Claude is waiting for approval to run a command",
      "home.phone.r1": "Sign-in fix",
      "home.phone.r1s": "Waiting",
      "home.phone.r1d": "Waiting for approval",
      "home.phone.r2": "API cleanup",
      "home.phone.r2s": "Running",
      "home.phone.r2d": "2. Sub-agent · tests",
      "home.phone.r3": "Docs site",
      "home.phone.r3s": "Done",
      "home.phone.r3d": "Fixed the guide",
      "home.phone.ask": "Run this command?",
      "home.phone.askFrom": "Claude · Sign-in fix",
      "home.phone.deny": "Deny",
      "home.phone.allow": "Allow",
      "home.bento.title": "Every detail, taken care of.",
      "home.plan.t": "Styles and plan cards",
      "home.plan.d": "Work in a set order with Ouroboros or Claude Plan. Read the plan, then approve it or ask for changes.",
      "home.pet.t": "Mighty Pet",
      "home.pet.d": "Progress and approvals from a corner of your screen.",
      "home.queue.t": "Request queue",
      "home.queue.d": "Keep talking while the agent works.",
      "home.queue.q1": "Match dark mode too",
      "home.queue.q2": "One more test",
      "home.queue.q3": "Update the README",
      "home.search.t": "Conversation search",
      "home.search.d": "Find that one line from an old conversation.",
      "home.search.q": "forgot password link",
      "home.dash.t": "Work status",
      "home.dash.d": "Every pane in every workspace, sorted by status, with account usage.",
      "home.term.t": "Terminal · files",
      "home.term.d": "Ghostty on Mac, PowerShell on Windows. Source, Markdown and image previews with diffs.",
      "home.remote.t": "Open another computer",
      "home.remote.d": "Open the workspaces of another Mac or Windows PC with a pairing code. The first connection needs that computer’s OK.",
      "home.remote.here": "This Mac",
      "home.remote.there": "Studio PC",
      "home.review.t": "Review queue",
      "home.review.d": "Go through the files an agent changed, one by one, and open any of them in your editor.",
      "home.review.open": "Open in editor",
      "home.final.title": "Get started.",
      "hero.meta": "macOS · Windows · Android",
      "dl.title": "Download & Install",
      "dl.versionPrefix": "Version ",
      "dl.sub": "Download the package for your OS. Future updates are verified by signature and staged automatically inside the app.",
      "dl.mac.desc": "macOS 14 or later (Apple Silicon). Open the DMG and drag Mighty Code to Applications.",
      "dl.mac.dmg": "Mac App (MightyCode-macos.dmg)",
      "dl.mac.zip": "Mac App (MightyCode-macos.zip)",
      "dl.mac.badge": "Apple Notarized · Opens without security warnings",
      "dl.win.tag": "Beta · x64 / ARM64",
      "dl.win.desc": "Windows 10 (2004+) or Windows 11. Installs per-user without requiring administrator privileges.",
      "dl.win.x64": "Windows Installer (x64)",
      "dl.win.arm64": "Windows Installer (ARM64)",
      "dl.and.desc": "Install the APK on your Android phone and pair via QR code in Settings → Mobile Remote.",
      "dl.and.apk": "Android App (MightyCode-android.apk)",
      "dl.and.guide": "Phone Remote Help →",
      "about.overview.title": "Overview",
      "about.overview.desc": "<b>Mighty Code</b> opens multiple panes per project folder (workspace) and runs agent CLIs (Claude Code, Codex, Antigravity, pi, opencode, Hermes) directly, keeping conversations and settings separate per pane. Available in Korean, English, Simplified Chinese, and Japanese.",
      "about.arch.title": "Architecture (Rust + Tauri 2)",
      "about.arch.desc": "The macOS and Windows apps are built from a single Rust + Tauri 2 workspace (<code>native/tauri</code>). Platform-independent crates (<code>mighty-core</code>, <code>mighty-run</code>, <code>mighty-graph</code>, <code>mighty-delegation</code>, <code>mighty-remote</code>, <code>mighty-tools</code>) are shared, while OS-specific implementations plug into trait ports (<code>mighty-platform</code>).",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code> (AppKit). Uses a native <b>Ghostty</b> (libghostty C API, Metal) terminal view; web pages open in your default browser. macOS 14+, Apple Silicon.",
      "about.arch.win": "Rust + Tauri 2 + WebView2. Uses Rust <b>ConPTY + xterm.js</b> for terminals, Job Objects for process lifecycle, and DPAPI for secrets. Windows 10 (2004+) or Windows 11, x64 · ARM64.",
      "about.arch.mob": "React Native (Expo) Android app. Connects via end-to-end encryption (X25519 → HKDF → ChaCha20-Poly1305) over the relay server, or directly over Tailscale.",
      "about.sec.title": "Principles & Scope",
      "about.sec.list": "<li>The app never runs <code>sudo</code> and never asks for your system password.</li><li>The relay forwards only ciphertext and knows no secrets; file-based styles require user inspection before use.</li><li>Auto-updates verify the Ed25519 signature on the release manifest before installing.</li><li>Agent panes other than Claude (Codex, Antigravity, pi, opencode, Hermes) carry a Beta badge.</li><li><code>pi</code>, <code>opencode</code>, and <code>Hermes</code> panes are not yet supported in Mighty Orchestration or remote workspaces.</li>",
      "about.oss.title": "Repository & Builds",
      "about.oss.desc": "Source code, relay deployment scripts (Cloudflare Workers / Oracle Cloud), help sources, and CI workflows are available on GitHub.",
      "about.oss.mac": "macOS Build ↗",
      "about.oss.win": "Windows Build ↗"
    },
    zh: {
      "meta.title.main": "Mighty Code",
      "meta.title.about": "关于 — Mighty Code",
      "nav.main": "首页",
      "nav.docs": "文档",
      "nav.about": "关于",
      "nav.download": "下载",
      "home.hero.title": "六个智能体，<br>一个窗口。",
      "home.hero.lead": "并排运行 Claude Code、Codex、Antigravity、pi、opencode 和 Hermes，以方块流程查看它们正在做的事。",
      "home.hero.how": "了解工作方式 ›",
      "home.hero.alt": "Mighty Code 界面：侧边栏与智能体、终端窗格",
      "home.agents.title": "直接使用你已有的智能体 CLI",
      "home.statement": "不必再读终端日志，<br>每个智能体在做什么，<br>一目了然。",
      "home.more": "了解更多 ›",
      "home.st.running": "运行中",
      "home.st.done": "完成",
      "home.st.writing": "撰写中…",
      "home.flow.kicker": "Mighty 视图",
      "home.flow.title": "从请求到结果，<br>一条流程。",
      "home.flow.s1.t": "发送请求。",
      "home.flow.s1.d": "运行期间也可以把下一个请求放进队列。",
      "home.flow.s2.t": "步骤以方块展开。",
      "home.flow.s2.d": "子智能体和后台命令出现时依次接上。",
      "home.flow.s3.t": "视图移到结果。",
      "home.flow.s3.d": "最终结果连同修改的行到达后，画面平滑移到该方块。",
      "home.flow.req": "请求 7 · Claude",
      "home.flow.reqText": "在登录界面加上“忘记密码”链接",
      "home.flow.step1": "1. 查找界面代码",
      "home.flow.step2": "2. 子智能体 · 测试",
      "home.flow.result": "结果",
      "home.flow.resultText": "已添加链接，4 个测试全部通过。",
      "home.orch.kicker": "Mighty 编排",
      "home.orch.title": "不必等待，<br>分头去做。",
      "home.orch.s1.t": "用 ⌘Enter 发到旁边的窗格。",
      "home.orch.s1.d": "运行中时，可选择放入队列，或交给某个智能体的独立 worktree。",
      "home.orch.s2.t": "每个子窗格在自己的分支上工作。",
      "home.orch.s2.d": "各自拥有 git worktree <code>mighty/&lt;id&gt;</code>，互不冲突。",
      "home.orch.s3.t": "检查后合并。",
      "home.orch.s3.d": "确认修改后，合并到父分支或丢弃。",
      "home.orch.parent": "父 · Claude",
      "home.orch.merged": "已合并 · 3",
      "home.num.agents": "个智能体 CLI 并排使用",
      "home.num.unit": "个",
      "home.num.queue": "每个窗格可排队的请求",
      "home.num.e2e": "与手机往来的所有内容均端到端加密",
      "home.phone.kicker": "手机远程",
      "home.phone.title": "人不在座位上，<br>工作也不停。",
      "home.phone.desc": "需要审批时手机会收到通知，看完内容即可当场回复。新设备只有在电脑上亲自允许后才能连接。",
      "home.phone.lock": "中继仅转发密文 · X25519 · ChaCha20-Poly1305",
      "home.phone.notifTitle": "Mighty Code · 修复登录",
      "home.phone.notif": "Claude 正在等待运行命令的批准",
      "home.phone.r1": "修复登录",
      "home.phone.r1s": "等待",
      "home.phone.r1d": "等待批准",
      "home.phone.r2": "整理 API",
      "home.phone.r2s": "运行中",
      "home.phone.r2d": "2. 子智能体 · 测试",
      "home.phone.r3": "文档网站",
      "home.phone.r3s": "完成",
      "home.phone.r3d": "已修改指南",
      "home.phone.ask": "要运行这条命令吗？",
      "home.phone.askFrom": "Claude · 修复登录",
      "home.phone.deny": "拒绝",
      "home.phone.allow": "批准",
      "home.bento.title": "细节之处，都已考虑。",
      "home.plan.t": "样式与计划卡片",
      "home.plan.d": "用 Ouroboros 或 Claude 计划按固定顺序推进。阅读计划后批准或要求修改。",
      "home.pet.t": "Mighty Pet",
      "home.pet.d": "在屏幕一角查看进度和审批请求。",
      "home.queue.t": "请求队列",
      "home.queue.d": "运行中也能继续提出请求。",
      "home.queue.q1": "暗色模式也一起改",
      "home.queue.q2": "再加一个测试",
      "home.queue.q3": "更新 README",
      "home.search.t": "对话搜索",
      "home.search.d": "立刻找到旧对话里的那一行。",
      "home.search.q": "忘记密码链接",
      "home.dash.t": "工作状态",
      "home.dash.d": "所有工作区的窗格按状态排列，并显示账户用量。",
      "home.term.t": "终端 · 文件",
      "home.term.d": "Mac 用 Ghostty，Windows 用 PowerShell。预览源码、Markdown、图片并比较修改。",
      "home.remote.t": "打开另一台电脑",
      "home.remote.d": "用配对码直接打开另一台 Mac 或 Windows 的工作区。首次连接需要对方电脑允许。",
      "home.remote.here": "这台 Mac",
      "home.remote.there": "工作室 PC",
      "home.review.t": "审查队列",
      "home.review.d": "依次查看智能体修改的文件，并在你的编辑器中直接打开。",
      "home.review.open": "在编辑器中打开",
      "home.final.title": "立即开始。",
      "hero.meta": "macOS · Windows · Android",
      "dl.title": "下载与安装",
      "dl.versionPrefix": "版本 ",
      "dl.sub": "下载适合您操作系统的文件。安装后，新版本会在应用内验证签名后自动下载准备。",
      "dl.mac.desc": "macOS 14 及以上 (Apple Silicon)。打开 DMG 并将 Mighty Code 拖入“应用程序”文件夹。",
      "dl.mac.dmg": "Mac 应用 (MightyCode-macos.dmg)",
      "dl.mac.zip": "Mac 应用 (MightyCode-macos.zip)",
      "dl.mac.badge": "已通过 Apple 公证 · 打开时无安全警告",
      "dl.win.tag": "测试版 · x64 / ARM64",
      "dl.win.desc": "Windows 10 (版本 2004+) 或 Windows 11。无需管理员权限，仅安装到当前用户目录。",
      "dl.win.x64": "Windows 安装程序 (x64)",
      "dl.win.arm64": "Windows 安装程序 (ARM64)",
      "dl.and.desc": "下载安装至手机，在电脑的“设置 → 移动远程”中扫描二维码连接。",
      "dl.and.apk": "Android 应用 (MightyCode-android.apk)",
      "dl.and.guide": "手机远程帮助 →",
      "about.overview.title": "概述",
      "about.overview.desc": "<b>Mighty Code</b> 在每个项目文件夹（工作区）打开多个运行窗格，每个窗格直接运行智能体 CLI（Claude Code、Codex、Antigravity、pi、opencode、Hermes），并独立维护对话与设置。界面支持韩语、英语、简体中文与日语。",
      "about.arch.title": "架构 (Rust + Tauri 2)",
      "about.arch.desc": "macOS 与 Windows 应用由同一个 Rust + Tauri 2 工作区 (<code>native/tauri</code>) 构建。平台无关的通用逻辑共享同一套 crate，操作系统特定功能则通过端口 (<code>mighty-platform</code>) 接入各自实现。",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code> (AppKit)。终端挂载原生 <b>Ghostty</b> (libghostty C API, Metal) 视图，网页在默认浏览器中打开。macOS 14+，Apple Silicon。",
      "about.arch.win": "Rust + Tauri 2 + WebView2。终端使用 Rust <b>ConPTY + xterm.js</b>，进程管理使用 Job Object，密钥存储使用 DPAPI。Windows 10 (2004+) 或 Windows 11，x64·ARM64。",
      "about.arch.mob": "React Native (Expo) Android 应用。通过中继服务器进行端到端加密 (X25519 → HKDF → ChaCha20-Poly1305) 连接，同一 Tailscale 网络下直接连接电脑。",
      "about.sec.title": "运行原则与支持范围",
      "about.sec.list": "<li>应用不使用 <code>sudo</code>，也不索取系统密码。</li><li>中继仅转发密文，不知晓任何机密；文件样式须确认内容并允许后方可使用。</li><li>自动更新在验证清单的 Ed25519 签名后才会安装。</li><li>除 Claude 外的智能体窗格（Codex、Antigravity、pi、opencode、Hermes）带有测试版标记。</li><li><code>pi</code>、<code>opencode</code>、<code>Hermes</code> 窗格暂不支持 Mighty 编排与远程工作区。</li>",
      "about.oss.title": "代码仓库与构建",
      "about.oss.desc": "源代码、中继部署脚本（Cloudflare Workers / Oracle Cloud）、帮助原文及 CI 工作流均在 GitHub 仓库公开。",
      "about.oss.mac": "macOS 构建 ↗",
      "about.oss.win": "Windows 构建 ↗"
    },
    ja: {
      "meta.title.main": "Mighty Code",
      "meta.title.about": "About — Mighty Code",
      "nav.main": "ホーム",
      "nav.docs": "ドキュメント",
      "nav.about": "About",
      "nav.download": "ダウンロード",
      "home.hero.title": "六つのエージェントを、<br>一つのウィンドウで。",
      "home.hero.lead": "Claude Code、Codex、Antigravity、pi、opencode、Hermes を並べて動かし、していることをブロックの流れで見守れます。",
      "home.hero.how": "しくみを見る ›",
      "home.hero.alt": "Mighty Code の画面：サイドバーとエージェント・ターミナルペイン",
      "home.agents.title": "いつものエージェント CLI をそのまま",
      "home.statement": "ターミナルのログを読む代わりに、<br>エージェントがいま何をしているかを<br>ひと目で。",
      "home.more": "詳しく見る ›",
      "home.st.running": "実行中",
      "home.st.done": "完了",
      "home.st.writing": "作成中…",
      "home.flow.kicker": "マイティ表示",
      "home.flow.title": "リクエストから<br>結果まで、流れで。",
      "home.flow.s1.t": "リクエストを送ります。",
      "home.flow.s1.d": "実行中でも次のリクエストをキューに積んでおけます。",
      "home.flow.s2.t": "ステップがブロックで育ちます。",
      "home.flow.s2.d": "サブエージェントやバックグラウンドのコマンドが現れるたびにつながります。",
      "home.flow.s3.t": "結果へ画面が移ります。",
      "home.flow.s3.d": "変更行とともに最終結果が届くと、そのブロックへなめらかに移動します。",
      "home.flow.req": "リクエスト 7 · Claude",
      "home.flow.reqText": "ログイン画面に「パスワードを忘れた」リンクを付けて",
      "home.flow.step1": "1. 画面のコードを探す",
      "home.flow.step2": "2. サブエージェント · テスト",
      "home.flow.result": "結果",
      "home.flow.resultText": "リンクを追加し、テスト 4 件がすべて通りました。",
      "home.orch.kicker": "マイティ・オーケストレーション",
      "home.orch.title": "待たずに、<br>任せましょう。",
      "home.orch.s1.t": "⌘Enter で隣のペインへ。",
      "home.orch.s1.d": "実行中なら、キューに送るか、どのエージェントの worktree に送るかを選べます。",
      "home.orch.s2.t": "子は自分のブランチで作業します。",
      "home.orch.s2.d": "それぞれが git worktree <code>mighty/&lt;id&gt;</code> を持つので、ぶつかりません。",
      "home.orch.s3.t": "確認して、マージ。",
      "home.orch.s3.d": "変更を確かめてから、親ブランチにマージするか破棄します。",
      "home.orch.parent": "親 · Claude",
      "home.orch.merged": "マージ完了 · 3",
      "home.num.agents": "並べて使えるエージェント CLI",
      "home.num.unit": "件",
      "home.num.queue": "ペインごとに積める次のリクエスト",
      "home.num.e2e": "スマホとやり取りする内容はすべてエンドツーエンド暗号化",
      "home.phone.kicker": "スマホリモート",
      "home.phone.title": "席を外しても、<br>作業は止まらない。",
      "home.phone.desc": "承認が必要になるとスマホに通知が届きます。内容を見てその場で答えましょう。新しい端末は、コンピュータで許可しない限りつながりません。",
      "home.phone.lock": "リレーは暗号文だけを中継します · X25519 · ChaCha20-Poly1305",
      "home.phone.notifTitle": "Mighty Code · ログイン修正",
      "home.phone.notif": "Claude がコマンド実行の承認を待っています",
      "home.phone.r1": "ログイン修正",
      "home.phone.r1s": "待機",
      "home.phone.r1d": "承認待ち",
      "home.phone.r2": "API 整理",
      "home.phone.r2s": "実行中",
      "home.phone.r2d": "2. サブエージェント · テスト",
      "home.phone.r3": "ドキュメントサイト",
      "home.phone.r3s": "完了",
      "home.phone.r3d": "ガイドを直しました",
      "home.phone.ask": "コマンドを実行しますか？",
      "home.phone.askFrom": "Claude · ログイン修正",
      "home.phone.deny": "拒否",
      "home.phone.allow": "承認",
      "home.bento.title": "細かいところまで、抜かりなく。",
      "home.plan.t": "スタイルとプランカード",
      "home.plan.d": "Ouroboros や Claude プランで決まった順に進めます。プランを読んで承認するか、修正を頼みましょう。",
      "home.pet.t": "Mighty Pet",
      "home.pet.d": "進行状況と承認依頼を画面の隅で。",
      "home.queue.t": "リクエストキュー",
      "home.queue.d": "実行中も話しかけ続けられます。",
      "home.queue.q1": "ダークモードも合わせて",
      "home.queue.q2": "テストをもう一つ",
      "home.queue.q3": "README を更新",
      "home.search.t": "会話検索",
      "home.search.d": "過去の会話のあの一行をすぐに。",
      "home.search.q": "パスワード リンク",
      "home.dash.t": "作業状況",
      "home.dash.d": "すべてのワークスペースのペインを状態順に、アカウント使用量まで。",
      "home.term.t": "ターミナル · ファイル",
      "home.term.d": "Mac は Ghostty、Windows は PowerShell。ソース・Markdown・画像のプレビューと差分比較も。",
      "home.remote.t": "別のコンピュータを開く",
      "home.remote.d": "ペアリングコードで別の Mac や Windows のワークスペースをそのまま開きます。最初の接続には相手のコンピュータの許可が必要です。",
      "home.remote.here": "この Mac",
      "home.remote.there": "仕事場の PC",
      "home.review.t": "レビューキュー",
      "home.review.d": "エージェントが変えたファイルを順に確認し、好きなエディタですぐ開けます。",
      "home.review.open": "エディタで開く",
      "home.final.title": "さあ、始めましょう。",
      "hero.meta": "macOS · Windows · Android",
      "dl.title": "ダウンロードとインストール",
      "dl.versionPrefix": "バージョン ",
      "dl.sub": "お使いの OS 用のファイルを受け取ります。インストール後、新しいバージョンはアプリ内で署名を確認した上で自動的に準備されます。",
      "dl.mac.desc": "macOS 14 以降 (Apple Silicon)。開いて Mighty Code をアプリケーションフォルダにドラッグします。",
      "dl.mac.dmg": "Mac アプリ (MightyCode-macos.dmg)",
      "dl.mac.zip": "Mac アプリ (MightyCode-macos.zip)",
      "dl.mac.badge": "Apple 公証済み · セキュリティ警告なしで開きます",
      "dl.win.tag": "ベータ · x64 / ARM64",
      "dl.win.desc": "Windows 10 (バージョン 2004 以降) または Windows 11。管理者権限なしで現在のユーザーにのみインストールされます。",
      "dl.win.x64": "Windows インストーラー (x64)",
      "dl.win.arm64": "Windows インストーラー (ARM64)",
      "dl.and.desc": "スマホにダウンロードしてインストールし、PC の「設定 → モバイルリモート」から QR コードで接続します。",
      "dl.and.apk": "Android アプリ (MightyCode-android.apk)",
      "dl.and.guide": "スマホリモート ヘルプ →",
      "about.overview.title": "概要",
      "about.overview.desc": "<b>Mighty Code</b> はプロジェクトフォルダ（ワークスペース）ごとに複数の実行ペインを開き、ペインごとにエージェント CLI（Claude Code、Codex、Antigravity、pi、opencode、Hermes）を直接実行して会話と設定を個別に維持するデスクトップアプリです。画面は韓国語・英語・中国語・日本語で表示できます。",
      "about.arch.title": "構造 (Rust + Tauri 2)",
      "about.arch.desc": "macOS アプリと Windows アプリは1つの Rust + Tauri 2 ワークスペース (<code>native/tauri</code>) で作られています。共通ロジックは OS コードなしで共有し、プラットフォームごとに異なる部分はポート (<code>mighty-platform</code>) に各 OS の実装を差し込みます。",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code> (AppKit)。ターミナルはネイティブ <b>Ghostty</b> (libghostty C API, Metal) ビューを載せ、Web ページは既定のブラウザで開きます。macOS 14 以降、Apple Silicon。",
      "about.arch.win": "Rust + Tauri 2 + WebView2。ターミナルは Rust <b>ConPTY + xterm.js</b> を使い、プロセス管理は Job Object、秘密値の保存は DPAPI を使います。Windows 10 (2004) 以降または Windows 11、x64・ARM64。",
      "about.arch.mob": "React Native (Expo) Android アプリ。リレーサーバーを経由してエンドツーエンド暗号化 (X25519 → HKDF → ChaCha20-Poly1305) で接続し、同じ Tailscale ネットワークなら PC に直接接続します。",
      "about.sec.title": "動作原則と対応範囲",
      "about.sec.list": "<li>アプリは <code>sudo</code> を使わず、システムパスワードを受け取りません。</li><li>リレーは暗号文のみを伝達し秘密を知りません。ファイルのスタイルは内容を確認して許可した後にのみ使います。</li><li>自動アップデートはマニフェストの Ed25519 署名を確認した後にのみインストールします。</li><li>Claude を除くエージェントペイン（Codex、Antigravity、pi、opencode、Hermes）にはベータ表示が付きます。</li><li><code>pi</code>、<code>opencode</code>、<code>Hermes</code> ペインはまだマイティ・オーケストレーションとリモートワークスペースでは使えません。</li>",
      "about.oss.title": "リポジトリとビルド",
      "about.oss.desc": "ソースコード、リレー（Cloudflare Workers / Oracle Cloud デプロイスクリプト）、ヘルプ原文、CI ワークフローは GitHub リポジトリにあります。",
      "about.oss.mac": "macOS ビルド ↗",
      "about.oss.win": "Windows ビルド ↗"
    }
  };

  function detectInitialLang() {
    const saved = localStorage.getItem("mc-lang");
    if (saved && I18N[saved]) return saved;
    const nav = (navigator.language || "ko").toLowerCase();
    if (nav.startsWith("ko")) return "ko";
    if (nav.startsWith("zh")) return "zh";
    if (nav.startsWith("ja")) return "ja";
    return "en";
  }

  let currentLang = detectInitialLang();
  let latestVersion = null;

  function renderVersionBadge() {
    const badge = document.getElementById("dl-version-badge");
    if (!badge || !latestVersion) return;
    const prefix = (I18N[currentLang] || I18N.ko)["dl.versionPrefix"] || "";
    badge.textContent = prefix + latestVersion;
  }

  function applyLang(lang) {
    if (!I18N[lang]) lang = "ko";
    currentLang = lang;
    localStorage.setItem("mc-lang", lang);
    document.documentElement.setAttribute("lang", lang === "zh" ? "zh-Hans" : lang);

    const dict = I18N[lang];
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const k = el.getAttribute("data-i18n");
      if (dict[k] !== undefined) el.textContent = dict[k];
    });
    document.querySelectorAll("[data-i18n-html]").forEach((el) => {
      const k = el.getAttribute("data-i18n-html");
      if (dict[k] !== undefined) el.innerHTML = dict[k];
    });

    document.querySelectorAll("[data-lang-btn]").forEach((btn) => {
      btn.classList.toggle("active", btn.getAttribute("data-lang-btn") === lang);
    });

    document.querySelectorAll("img[data-lang-img]").forEach((img) => {
      const file = img.getAttribute("data-lang-img");
      img.src = `${lang}/img/${file}`;
    });

    // The help is the app's own help build, copied in by scripts/site/sync-help.sh.
    const docsHref = `help/${lang}/`;
    document.querySelectorAll("[data-docs-link]").forEach((a) => {
      a.setAttribute("href", docsHref);
    });
    document.querySelectorAll("[data-docs-hash]").forEach((a) => {
      a.setAttribute("href", docsHref + a.getAttribute("data-docs-hash"));
    });

    renderVersionBadge();
    // home.js draws words of its own (block states, the statement's lit words); it reads them here and redraws on this event
    document.dispatchEvent(new CustomEvent("mc-lang", { detail: lang }));
  }

  window.mcT = (key) => {
    const dict = I18N[currentLang] || I18N.ko;
    return dict[key] !== undefined ? dict[key] : (I18N.ko[key] || "");
  };
  window.mcLang = () => currentLang;

  document.querySelectorAll("[data-lang-btn]").forEach((btn) => {
    btn.addEventListener("click", () => {
      applyLang(btn.getAttribute("data-lang-btn"));
    });
  });

  applyLang(currentLang);

  // ── Dynamic latest.json Fetch & Download Link Binding ──────────────────────
  function parseManifestEnvelope(raw) {
    if (!raw || typeof raw !== "object") return null;
    if (typeof raw.payload === "string") {
      try {
        const bytes = Uint8Array.from(atob(raw.payload), (c) => c.charCodeAt(0));
        const inner = JSON.parse(new TextDecoder().decode(bytes));
        if (inner && typeof inner === "object") return inner;
      } catch (_) {}
    }
    return raw;
  }

  function applyLatestManifest(raw) {
    const manifest = parseManifestEnvelope(raw);
    if (!manifest) return;
    const ver = (manifest.version || raw.version || "").replace(/^v/i, "").trim();
    if (!ver) return;

    latestVersion = ver;
    renderVersionBadge();

    const macZipUrl = (manifest.macos && manifest.macos.url) || `${PUBLIC_BASE}/${ver}/MightyCode-macos.zip`;
    const versionBase = macZipUrl.replace(/\/[^/]+$/, "");
    const winX64ZipUrl =
      (manifest.windows && manifest.windows.x64 && manifest.windows.x64.url) ||
      `${versionBase}/MightyCode-windows-x64.zip`;
    const winArm64ZipUrl =
      (manifest.windows && manifest.windows.arm64 && manifest.windows.arm64.url) ||
      `${versionBase}/MightyCode-windows-arm64.zip`;

    const links = {
      "macos-dmg": `${versionBase}/MightyCode-macos.dmg`,
      "macos-zip": macZipUrl,
      "win-x64-exe": `${versionBase}/MightyCode-windows-x64-setup.exe`,
      "win-arm64-exe": `${versionBase}/MightyCode-windows-arm64-setup.exe`,
      "win-x64-zip": winX64ZipUrl,
      "win-arm64-zip": winArm64ZipUrl,
      "android-apk": `${PUBLIC_BASE}/android/MightyCode-android.apk`
    };

    Object.entries(links).forEach(([key, url]) => {
      document.querySelectorAll(`[data-dl-asset="${key}"]`).forEach((a) => {
        a.setAttribute("href", url);
      });
    });
  }

  async function loadLatestVersion() {
    const isLocal = location.hostname === "localhost" || location.hostname === "127.0.0.1";
    const candidates = isLocal
      ? ["latest.json", `${PUBLIC_BASE}/latest.json`]
      : ["/api/latest", "latest.json", `${PUBLIC_BASE}/latest.json`];
    for (const url of candidates) {
      try {
        const res = await fetch(url, { cache: "no-store" });
        if (!res.ok) continue;
        const json = await res.json();
        if (json && (json.version || json.payload)) {
          applyLatestManifest(json);
          return;
        }
      } catch (_) {}
    }
  }

  loadLatestVersion();

  // ── Theme Toggle ───────────────────────────────────────────────────────────
  const root = document.documentElement;
  const savedTheme = localStorage.getItem("mc-theme");
  if (savedTheme === "light" || savedTheme === "dark") {
    root.setAttribute("data-theme", savedTheme);
  }

  document.querySelectorAll("[data-theme-toggle]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
      root.setAttribute("data-theme", next);
      localStorage.setItem("mc-theme", next);
      localStorage.setItem("mc-help-theme", next);
    });
  });

  // ── Screen Viewer Tabs ─────────────────────────────────────────────────────
  const tabs = document.querySelectorAll("[data-view-tab]");
  const panes = document.querySelectorAll("[data-view-pane]");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const key = tab.getAttribute("data-view-tab");
      tabs.forEach((t) => t.classList.toggle("active", t === tab));
      panes.forEach((p) => p.classList.toggle("active", p.getAttribute("data-view-pane") === key));
    });
  });
})();
