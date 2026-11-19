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
      "hero.meta": "macOS · Windows · Android",
      "hero.desc": "<b>Claude Code · Codex · Antigravity · pi · opencode · Hermes를 한 창에서 나란히 쓰는 데스크톱 앱입니다.</b><br>프로젝트 폴더(워크스페이스)마다 여러 실행 창을 열고, 창마다 에이전트 CLI를 직접 실행해 대화와 설정을 따로 유지합니다. 요청·하위 에이전트·백그라운드 작업을 블록 흐름으로 보여 주는 <b>마이티 보기</b>, 정해진 순서로 일을 진행하는 <b>스타일</b>(오로보로스·클러드 플랜), 일을 자기 git worktree를 가진 자식 창에 나눠 맡기는 <b>마이티 오케스트레이션</b>, 다른 컴퓨터의 워크스페이스를 여는 <b>원격 연결(Tailscale)</b>, 휴대폰에서 상태를 보고 답하는 <b>휴대폰 원격</b>이 있습니다.",
      "hero.btn.download": "다운로드",
      "hero.btn.docs": "도움말 · 문서",
      "tab.overview": "화면 구성",
      "tab.mighty": "마이티 보기",
      "tab.delegation": "마이티 오케스트레이션",
      "tab.plan": "스타일과 계획 카드",
      "tab.dashboard": "작업 현황",
      "tab.mobile": "휴대폰 원격",
      "cap.overview": "<b>화면 구성:</b> 사이드바에서 워크스페이스와 실행 창을 고르고, 오른쪽 작업 영역에 에이전트·터미널·파일·브라우저 창을 탭이나 분할로 배치합니다.",
      "cap.mighty": "<b>마이티 보기:</b> 요청 → 그 사이에 일어난 일(하위 에이전트·백그라운드 명령) → 최종 결과와 바꾼 줄을 블록 흐름으로 보여 줍니다.",
      "cap.mighty.code": "흐름 | 기록 | 파이프라인",
      "cap.delegation": "<b>마이티 오케스트레이션:</b> 자식은 저마다 자기 git worktree와 브랜치(<code>mighty/&lt;자식 id&gt;</code>)에서 일하고, 끝나면 사이드바 카드에서 부모 브랜치에 병합하거나 버립니다.",
      "cap.plan": "<b>스타일과 계획:</b> 클러드 플랜은 새 요청마다 계획 모드로 시작하고, 계획 카드의 네 가지 답(자동 편집 승인 / 매번 확인 승인 / 수정 요청 / 취소)으로 진행합니다.",
      "cap.dashboard": "<b>작업 현황:</b> 모든 워크스페이스의 실행 창을 상태 순으로 모으고, 실행 중·응답 대기·완료 개수와 계정 사용량을 보여 줍니다.",
      "cap.mobile": "<b>휴대폰 원격:</b> QR 코드로 휴대폰을 연결해 상태를 보고 승인·질문·계획에 답합니다. 릴레이는 암호문만 전달하며 같은 Tailscale 네트워크면 직접 연결합니다.",
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
      "feat.title": "주요 기능",
      "feat.sub": "각 기능의 자세한 사용법은 문서(도움말)에 있습니다.",
      "feat.f1.title": "에이전트 창과 마이티 보기",
      "feat.f1.desc": "실행 중에도 다음 요청을 대기열(창당 16개)에 넣을 수 있습니다. 모든 에이전트 창은 요청 → 그 사이에 일어난 일 → 최종 결과를 블록 흐름으로 보여 주고, 흐름·기록·파이프라인 보기로 전환합니다.",
      "feat.f2.title": "마이티 오케스트레이션",
      "feat.f2.desc": "에이전트 창이 일하는 동안 다른 일을 옆의 자식 창에 맡깁니다(⌘Enter 또는 delegate 도구). 자식은 저마다 자기 git worktree(mighty/<자식 id>)에서 일하고, 끝나면 부모 브랜치에 병합하거나 버립니다. 자식은 Claude, Codex, Antigravity로 실행할 수 있습니다.",
      "feat.f3.title": "스타일과 계획 카드",
      "feat.f3.desc": "Claude 창에서 입력창 위 메뉴로 오로보로스(Ouroboros)와 클러드 플랜 스타일을 고릅니다. 클러드 플랜은 새 요청마다 계획 모드로 시작하며, 계획 카드와 데스크톱 펫에서 승인·수정 요청·취소를 고릅니다.",
      "feat.f4.title": "승인·질문·로그인 복구",
      "feat.f4.desc": "권한 밖의 일은 승인 카드로, 질문은 선택 요청 카드로 묻습니다. Antigravity는 거절한 작업을 턴 끝에 모아 묻고, 허용하면 같은 대화를 이어갑니다. CLI 로그인이 만료되면 복구 카드에서 다시 로그인하고 실패한 요청을 다시 보냅니다.",
      "feat.f5.title": "터미널·파일·브라우저 창",
      "feat.f5.desc": "터미널 창(⌘T)은 워크스페이스 폴더에서 열리는 실제 터미널입니다(Mac은 Ghostty, Windows는 PowerShell). 파일 창(⇧⌘E)은 소스·Markdown·이미지·PDF를 미리 보고 변경점을 비교합니다.",
      "feat.f6.title": "원격 연결(Tailscale)과 휴대폰 원격",
      "feat.f6.desc": "Mac이나 Windows 두 대를 Tailscale로 이어 다른 컴퓨터의 워크스페이스를 열거나, QR 코드로 휴대폰을 연결해 종단 간 암호화(릴레이 또는 Tailscale 직접 연결)로 상태를 보고 답합니다.",
      "about.overview.title": "개요",
      "about.overview.desc": "<b>Mighty Code</b>는 프로젝트 폴더(워크스페이스)마다 여러 실행 창을 열고, 창마다 에이전트 CLI(Claude Code, Codex, Antigravity, pi, opencode, Hermes)를 직접 실행해 대화와 설정을 따로 유지하는 데스크톱 앱입니다. 화면은 한국어·영어·중국어·일본어로 볼 수 있습니다.",
      "about.arch.title": "구조 (Rust + Tauri 2)",
      "about.arch.desc": "macOS 앱과 Windows 앱은 하나의 Rust + Tauri 2 워크스페이스(<code>native/tauri</code>)로 만들어집니다. 공통 로직(<code>mighty-core</code>, <code>mighty-run</code>, <code>mighty-graph</code>, <code>mighty-delegation</code>, <code>mighty-remote</code>, <code>mighty-tools</code>)은 OS 코드 없이 공유하고, 플랫폼마다 다른 부분은 포트(<code>mighty-platform</code>)에 각 운영체제 구현을 끼워 넣습니다.",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code>(AppKit). 터미널은 네이티브 <b>Ghostty</b>(libghostty C API, Metal) 뷰를 올리고, 브라우저는 CEF와 WKWebView 중 설정으로 고릅니다. macOS 14 이상, Apple Silicon.",
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
      "hero.meta": "macOS · Windows · Android",
      "hero.desc": "<b>A desktop app for running Claude Code · Codex · Antigravity · pi · opencode · Hermes side by side in one window.</b><br>Open multiple panes per project folder (workspace), each running an agent CLI directly with its own conversation and settings. Includes <b>Mighty View</b> block timelines, structured <b>Styles</b> (Ouroboros · Claude Plan), <b>Mighty Orchestration</b> with isolated Git worktree child panes, <b>Remote Workspaces (Tailscale)</b>, and end-to-end encrypted <b>Phone Remote</b>.",
      "hero.btn.download": "Download",
      "hero.btn.docs": "Help & Docs",
      "tab.overview": "Layout",
      "tab.mighty": "Mighty View",
      "tab.delegation": "Mighty Orchestration",
      "tab.plan": "Styles & Plan Card",
      "tab.dashboard": "Dashboard",
      "tab.mobile": "Phone Remote",
      "cap.overview": "<b>Layout:</b> Switch workspaces and panes in the sidebar, and arrange agent, terminal, file, and browser panes as tabs or splits.",
      "cap.mighty": "<b>Mighty View:</b> Shows Request → Intermediate steps (subagents & background commands) → Final result and changed lines as a block flow.",
      "cap.mighty.code": "Flow | Log | Pipeline",
      "cap.delegation": "<b>Mighty Orchestration:</b> Each child pane works in its own git worktree and branch (<code>mighty/&lt;child-id&gt;</code>), then merges into the parent branch or is discarded from its card.",
      "cap.plan": "<b>Styles & Plan:</b> Claude Plan starts every request in plan mode and proceeds via four card actions (Approve auto-edit / Approve step-by-step / Request changes / Cancel).",
      "cap.dashboard": "<b>Dashboard:</b> Collects all panes across workspaces by status, showing Running, Waiting, Done counts and CLI account usage.",
      "cap.mobile": "<b>Phone Remote:</b> Pair your phone via QR code to check status and answer approvals, questions, and plans. End-to-end encrypted via relay or direct over Tailscale.",
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
      "feat.title": "Core Features",
      "feat.sub": "Detailed usage instructions for each feature are available in the documentation.",
      "feat.f1.title": "Agent Panes & Mighty View",
      "feat.f1.desc": "Queue up to 16 follow-up requests while an agent is running. Every agent pane renders Request → Steps → Result as a block flow, switchable between Flow, Log, and Pipeline views.",
      "feat.f2.title": "Mighty Orchestration",
      "feat.f2.desc": "Delegate work to child panes while an agent is busy (⌘Enter or the delegate MCP tool). Each child works in its own git worktree (mighty/<id>) and can run Claude, Codex, or Antigravity.",
      "feat.f3.title": "Styles & Plan Cards",
      "feat.f3.desc": "Select Ouroboros or Claude Plan from the style menu above the composer. Review plans and approve or request revisions from the card or desktop pet.",
      "feat.f4.title": "Approvals, Questions & Login Recovery",
      "feat.f4.desc": "Out-of-scope actions prompt an approval card; agent questions show a selection card. Antigravity batches denied actions at turn end. Expired CLI sessions offer one-click re-login and retry.",
      "feat.f5.title": "Terminal, File & Browser Panes",
      "feat.f5.desc": "Terminal panes (⌘T) open a real shell in the workspace folder (Ghostty on Mac, PowerShell on Windows). File panes (⇧⌘E) preview source, Markdown, images, PDFs, and diffs.",
      "feat.f6.title": "Remote (Tailscale) & Phone Remote",
      "feat.f6.desc": "Link two computers over Tailscale to open remote workspaces without copying files, or pair an Android phone via QR code over E2EE relay or Tailscale direct connection.",
      "about.overview.title": "Overview",
      "about.overview.desc": "<b>Mighty Code</b> opens multiple panes per project folder (workspace) and runs agent CLIs (Claude Code, Codex, Antigravity, pi, opencode, Hermes) directly, keeping conversations and settings separate per pane. Available in Korean, English, Simplified Chinese, and Japanese.",
      "about.arch.title": "Architecture (Rust + Tauri 2)",
      "about.arch.desc": "The macOS and Windows apps are built from a single Rust + Tauri 2 workspace (<code>native/tauri</code>). Platform-independent crates (<code>mighty-core</code>, <code>mighty-run</code>, <code>mighty-graph</code>, <code>mighty-delegation</code>, <code>mighty-remote</code>, <code>mighty-tools</code>) are shared, while OS-specific implementations plug into trait ports (<code>mighty-platform</code>).",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code> (AppKit). Uses a native <b>Ghostty</b> (libghostty C API, Metal) terminal view and configurable CEF or WKWebView browser panes. macOS 14+, Apple Silicon.",
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
      "hero.meta": "macOS · Windows · Android",
      "hero.desc": "<b>在同一窗口并排使用 Claude Code · Codex · Antigravity · pi · opencode · Hermes 的桌面应用。</b><br>每个项目文件夹（工作区）可打开多个运行窗格，每个窗格直接运行智能体 CLI 并独立保存对话与设置。包含以方块流程展示请求与子任务的<b>Mighty 视图</b>、按固定步骤推进的<b>样式</b>（Ouroboros·Claude 计划）、在各自独立 git worktree 子窗格中分担任务的<b>Mighty 编排</b>、打开其他电脑工作区的<b>远程连接 (Tailscale)</b> 以及<b>手机远程</b>。",
      "hero.btn.download": "下载",
      "hero.btn.docs": "帮助与文档",
      "tab.overview": "界面布局",
      "tab.mighty": "Mighty 视图",
      "tab.delegation": "Mighty 编排",
      "tab.plan": "样式与计划卡片",
      "tab.dashboard": "任务概况",
      "tab.mobile": "手机远程",
      "cap.overview": "<b>界面布局：</b>在侧边栏选择工作区与运行窗格，在右侧工作区以标签页或分屏方式排列智能体、终端、文件与浏览器窗格。",
      "cap.mighty": "<b>Mighty 视图：</b>将“请求 → 中间过程（子智能体与后台命令）→ 最终结果与变更行”以方块流程呈现。",
      "cap.mighty.code": "流程 | 记录 | 流水线",
      "cap.delegation": "<b>Mighty 编排：</b>每个子窗格在各自的 git worktree 与分支 (<code>mighty/&lt;子id&gt;</code>) 中工作，完成后可在侧边栏卡片中合并回父分支或丢弃。",
      "cap.plan": "<b>样式与计划：</b>Claude 计划在每次新请求时以计划模式启动，通过计划卡片的四个选项（批准并自动编辑 / 批准并每次确认 / 请求修改 / 取消）继续。",
      "cap.dashboard": "<b>任务概况：</b>按状态汇总所有工作区的运行窗格，显示运行中、等待响应、已完成数量与账号用量。",
      "cap.mobile": "<b>手机远程：</b>通过二维码连接手机查看状态并回复审批、提问与计划。中继仅转发密文，同一 Tailscale 网络下直接连接。",
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
      "feat.title": "主要功能",
      "feat.sub": "各项功能的详细用法请参阅帮助文档。",
      "feat.f1.title": "智能体窗格与 Mighty 视图",
      "feat.f1.desc": "运行期间也可将后续请求加入队列（每窗格最多 16 个）。所有智能体窗格均以方块流程展示请求与结果，支持切换流程、记录与流水线视图。",
      "feat.f2.title": "Mighty 编排",
      "feat.f2.desc": "当一个智能体窗格工作时，可将其他任务交给旁边的子窗格（⌘Enter 或 delegate 工具）。子窗格在独立的 git worktree (mighty/<id>) 中运行 Claude、Codex 或 Antigravity。",
      "feat.f3.title": "样式与计划卡片",
      "feat.f3.desc": "在 Claude 窗格输入框上方选择 Ouroboros 或 Claude 计划样式。支持在计划卡片或桌面宠物上直接批准、请求修改或取消。",
      "feat.f4.title": "审批、提问与登录恢复",
      "feat.f4.desc": "权限外的操作通过审批卡片询问，问题通过选择卡片回答。Antigravity 会在轮次结束时汇总被拒操作。CLI 登录过期时可通过恢复卡片重新登录并重发请求。",
      "feat.f5.title": "终端、文件与浏览器窗格",
      "feat.f5.desc": "终端窗格 (⌘T) 在工作区文件夹打开真实终端（Mac 为 Ghostty，Windows 为 PowerShell）。文件窗格 (⇧⌘E) 只读预览源码、Markdown、图片与 PDF。",
      "feat.f6.title": "远程连接 (Tailscale) 与手机远程",
      "feat.f6.desc": "通过 Tailscale 连接两台电脑直接打开远程工作区，或扫码配对手机通过端到端加密（中继或 Tailscale 直连）查看状态并回复。",
      "about.overview.title": "概述",
      "about.overview.desc": "<b>Mighty Code</b> 在每个项目文件夹（工作区）打开多个运行窗格，每个窗格直接运行智能体 CLI（Claude Code、Codex、Antigravity、pi、opencode、Hermes），并独立维护对话与设置。界面支持韩语、英语、简体中文与日语。",
      "about.arch.title": "架构 (Rust + Tauri 2)",
      "about.arch.desc": "macOS 与 Windows 应用由同一个 Rust + Tauri 2 工作区 (<code>native/tauri</code>) 构建。平台无关的通用逻辑共享同一套 crate，操作系统特定功能则通过端口 (<code>mighty-platform</code>) 接入各自实现。",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code> (AppKit)。终端挂载原生 <b>Ghostty</b> (libghostty C API, Metal) 视图，浏览器可在设置中选择 CEF 或 WKWebView。macOS 14+，Apple Silicon。",
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
      "hero.meta": "macOS · Windows · Android",
      "hero.desc": "<b>Claude Code · Codex · Antigravity · pi · opencode · Hermes を1つのウィンドウで並べて使うデスクトップアプリです。</b><br>プロジェクトフォルダ（ワークスペース）ごとに複数の実行ペインを開き、ペインごとにエージェント CLI を直接実行して会話と設定を個別に保ちます。リクエストやサブエージェント・バックグラウンド作業をブロックフローで表示する<b>マイティ表示</b>、決まった順序で作業を進める<b>スタイル</b>（Ouroboros・Claude プラン）、独自の git worktree を持つ子ペインに作業を分担する<b>マイティ・オーケストレーション</b>、別のコンピュータのワークスペースを開く<b>リモート接続 (Tailscale)</b>、スマホから状態を見て応答する<b>スマホリモート</b>を備えています。",
      "hero.btn.download": "ダウンロード",
      "hero.btn.docs": "ヘルプ・ドキュメント",
      "tab.overview": "画面構成",
      "tab.mighty": "マイティ表示",
      "tab.delegation": "マイティ・オーケストレーション",
      "tab.plan": "スタイルと計画カード",
      "tab.dashboard": "作業状況",
      "tab.mobile": "スマホリモート",
      "cap.overview": "<b>画面構成：</b>サイドバーでワークスペースと実行ペインを選び、右側の作業領域にエージェント・ターミナル・ファイル・ブラウザペインをタブや分割で配置します。",
      "cap.mighty": "<b>マイティ表示：</b>リクエスト → その間に起きたこと（サブエージェント・バックグラウンドコマンド）→ 最終結果と変更行をブロックフローで表示します。",
      "cap.mighty.code": "フロー | 履歴 | パイプライン",
      "cap.delegation": "<b>マイティ・オーケストレーション：</b>子はそれぞれ独自の git worktree とブランチ (<code>mighty/&lt;子id&gt;</code>) で作業し、完了後にサイドバーのカードから親ブランチへマージまたは破棄します。",
      "cap.plan": "<b>スタイルと計画：</b>Claude プランは新しいリクエストごとに計画モードで開始し、計画カードの4つの応答（自動編集で承認 / 都度確認で承認 / 修正依頼 / キャンセル）で進めます。",
      "cap.dashboard": "<b>作業状況：</b>全ワークスペースの実行ペインを状態順に集め、実行中・応答待ち・完了の数とアカウント使用量を表示します。",
      "cap.mobile": "<b>スマホリモート：</b>QRコードでスマホを接続し、状態の確認や承認・質問・計画への応答を行います。リレーは暗号文のみを中継し、同じ Tailscale ネットワークなら直接接続します。",
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
      "feat.title": "主な機能",
      "feat.sub": "各機能の詳しい使い方はドキュメント（ヘルプ）にあります。",
      "feat.f1.title": "エージェントペインとマイティ表示",
      "feat.f1.desc": "実行中でも次のリクエストをキュー（ペインあたり16件まで）に追加できます。すべてのエージェントペインはリクエスト → 途中経過 → 最終結果をブロックフローで表示し、フロー・履歴・パイプライン表示を切り替えられます。",
      "feat.f2.title": "マイティ・オーケストレーション",
      "feat.f2.desc": "エージェントペインの作業中に別の作業を隣の子ペインに任せます（⌘Enter または delegate ツール）。子は独自の git worktree (mighty/<id>) で作業し、Claude・Codex・Antigravity のいずれかで実行できます。",
      "feat.f3.title": "スタイルと計画カード",
      "feat.f3.desc": "Claude ペインの入力欄上のメニューから Ouroboros や Claude プランを選びます。計画カードやデスクトップペットから承認・修正依頼・キャンセルを選べます。",
      "feat.f4.title": "承認・質問・ログイン復旧",
      "feat.f4.desc": "権限外の作業は承認カードで、質問は選択カードで尋ねます。Antigravity は拒否した作業をターンの終わりにまとめて尋ねます。CLI のログインが切れた場合は復旧カードから再ログインしてリクエストを再送します。",
      "feat.f5.title": "ターミナル・ファイル・ブラウザペイン",
      "feat.f5.desc": "ターミナルペイン (⌘T) はワークスペースフォルダで開く実際のターミナルです（Mac は Ghostty、Windows は PowerShell）。ファイルペイン (⇧⌘E) はソース・Markdown・画像・PDF をプレビューします。",
      "feat.f6.title": "リモート接続 (Tailscale) とスマホリモート",
      "feat.f6.desc": "Mac や Windows 2台を Tailscale でつないで別の PC のワークスペースを開いたり、QR コードでスマホを接続してエンドツーエンド暗号化で状態確認と応答を行えます。",
      "about.overview.title": "概要",
      "about.overview.desc": "<b>Mighty Code</b> はプロジェクトフォルダ（ワークスペース）ごとに複数の実行ペインを開き、ペインごとにエージェント CLI（Claude Code、Codex、Antigravity、pi、opencode、Hermes）を直接実行して会話と設定を個別に維持するデスクトップアプリです。画面は韓国語・英語・中国語・日本語で表示できます。",
      "about.arch.title": "構造 (Rust + Tauri 2)",
      "about.arch.desc": "macOS アプリと Windows アプリは1つの Rust + Tauri 2 ワークスペース (<code>native/tauri</code>) で作られています。共通ロジックは OS コードなしで共有し、プラットフォームごとに異なる部分はポート (<code>mighty-platform</code>) に各 OS の実装を差し込みます。",
      "about.arch.mac": "Rust + Tauri 2 + <code>objc2</code> (AppKit)。ターミナルはネイティブ <b>Ghostty</b> (libghostty C API, Metal) ビューを載せ、ブラウザは CEF と WKWebView から設定で選びます。macOS 14 以降、Apple Silicon。",
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

    const docsHref = lang === "ko" ? "docs.html" : `${lang}/index.html`;
    document.querySelectorAll("[data-docs-link]").forEach((a) => {
      a.setAttribute("href", docsHref);
    });
    document.querySelectorAll("[data-docs-hash]").forEach((a) => {
      a.setAttribute("href", docsHref + a.getAttribute("data-docs-hash"));
    });

    renderVersionBadge();
  }

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
