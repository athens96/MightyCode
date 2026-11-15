# Mighty Code → Rust + Tauri 2 (0.5) — 변환 계획

작성 2026-11-13, 기준 0.4.59 (`2f3ca705`), 브랜치 `feat/tauri`. 근거 자료: `.omc/research/tauri/host-contract.md`, `core-catalog.md`, `mac-gaps.md`.

## 0. 목표와 완료 기준
- Windows 앱(C# Core 59k줄 + WinUI 호스트 41k줄)을 Rust + Tauri 2로 다시 만든다. 화면은 지금의 React 페이지(`native/windows/web`, 2만 줄)를 그대로 쓴다.
- 그다음 Mac 앱(SwiftUI + MightyCore 56k줄)을 같은 Tauri 앱으로 바꾼다. 휴대폰 앱은 바꾸지 않는다(통신 규약을 그대로 지킨다).
- **0.5 출시 조건:** (1) C# Core 테스트 1,170개를 Rust로 옮겨 모두 통과, (2) 공통 기대값 파일(`native/contracts/fixtures`) 전부 통과, (3) Windows 웹 smoke와 같은 항목의 Tauri smoke 통과, (4) 0.4.x에서 앱 안 업데이트로 0.5가 설치되고 데이터가 그대로 이어짐, (5) Mac도 같은 조건.

## 1. 지켜야 할 호환 (깨지면 사용자가 데이터·업데이트를 잃음)
| 항목 | 그대로 둘 것 |
|---|---|
| 상태 파일 | `%APPDATA%\MightyCode\workspace-state.json` v1, `.bak`, NUL 파일 처리, 8 MiB 제한, 같은 JSON 모양(camelCase, null 생략, 모르는 필드 보존, .NET `"O"` 시각 형식) |
| 다른 데이터 | `delegation-state.json`, `mobile-identity.json`, `remote-connections.json`(DPAPI), `toolkit.json`, `companion-settings.json`, `image-cache`, `file-originals`, HKCU `Software\MightyCode` 값 |
| 실행 파일 | 이름 `MightyCode.exe`, 설치 폴더 `%LOCALAPPDATA%\Programs\MightyCode`, Inno Setup AppId `{F2F99AEF-…}`(`.iss` 유지) |
| 업데이트 | 서명된 `latest.json`(envelope `mightyclaude-update-v1`, Ed25519, 공개키 `C02DD3q…`), 같은 R2 주소. Tauri 기본 업데이터(minisign)는 쓰지 않음 |
| **0.4.x → 0.5 업데이트 손넘김** | 0.4.x가 새 exe를 `--update-helper --staged --install --backup --package --sha256 --size --wait-pid --relaunch` 로 실행한다. Rust exe가 WebView2를 띄우기 전에 이 모드를 C#과 똑같이 처리해야 함(복구, pid 대기, sha256 재확인, 두 번 이름 바꾸기와 되돌리기, 새 환경으로 재실행, 뮤텍스, 레지스트리 DisplayVersion, install.log). **가장 큰 위험** — 안 되면 0.4.x 다리 버전을 먼저 낸다 |
| 실행 파일의 다른 모드 | `--agent-io-mcp`, `--agent-delegation-mcp`(CLI가 띄우는 MCP 서버), `--profile`, `--smoke-test`, `--smoke-exit`, `--help-capture`, `--perf-scenario`, `--fake-claude`, `--help-demo-cli` |
| 휴대폰·원격 | m1 API, 릴레이 암호(X25519→HKDF→ChaCha20-Poly1305), 푸시 봉투, 원격 작업 공간 v2(포트 43137, 상태 코드 순서), Tailscale 직접 연결 |

## 2. 구조
```
native/tauri/
  Cargo.toml                 워크스페이스
  crates/mighty-core/        상태·모델·Wire JSON·로케일·디자인 토큰 (플랫폼 무관)
  crates/mighty-platform/    프로세스(Job Object, .cmd), ConPTY/forkpty, 경로, DPAPI/Keychain, 레지스트리/UserDefaults
  crates/mighty-run/         실행 엔진, 공급자 인자, 스트림 파서(Claude/Codex/agy/pi/ACP), 권한, 계획·체크리스트
  crates/mighty-graph/       그래프 추적기, 마이티 흐름·타임라인·구조·역할 보기
  crates/mighty-remote/      휴대폰(릴레이·직접·m1·푸시), 원격 작업 공간 v2, 화면 공유 규약
  crates/mighty-delegation/  위임, 작업 트리, MCP 서버, Agent IO
  crates/mighty-tools/       플러그인, CLI 계정·업데이트, 설정 도구, 사용량, 상태 줄, 작업 지도, 스타일(동결 엔진)
  src-tauri/                 호스트: 브리지, 델타 엔진, 셸·작성창·카드 모델, native 폼(NativeNode 생성), 창들, 업데이트 도우미, smoke
  tests/fixtures → ../contracts/fixtures (공통 기대값 그대로)
```
- 페이지와 통신: `HostPort`만 Tauri용으로 바꾼다(`tauriPort.ts`). 결과와 push는 한 이벤트 채널로 순서대로.
- 지금 C# 호스트가 "보이지 않는 WinUI 컨트롤을 읽어" 만드는 것(셸 모델, 작성창 상태, 카드, 터미널 바닥, 설정·대화 상자)은 Rust에서 규칙을 직접 계산해 같은 JSON을 만든다. 설정·대화 상자는 같은 NativeNode 모양으로 Rust가 직접 만든다(페이지 수정 없음).
- 페이지 폴더는 Mac 단계 시작 때 `native/windows/web` → `native/web`으로 옮긴다(C# 빌드·CI 함께 수정).

## 3. 검증 방법 ("완벽하게 같게"를 무엇으로 확인하나)
1. **공통 기대값:** 58개 fixture 중 56개를 Core 테스트가 읽는다 → Rust 테스트가 같은 파일을 읽어 같은 결과를 내야 통과.
2. **C# 테스트 1:1 이전:** `Verification.cs`의 1,170개 검사를 이름 그대로 `cargo test`로 옮긴다. 가짜 CLI는 테스트 바이너리를 다시 실행하는 방식 그대로(`[[bin]] fake-cli`).
3. **차이 비교 도구:** 같은 입력(상태 파일, CLI 스트림 녹화)을 C#과 Rust에 넣고 출력 JSON(스냅샷, 델타, 셸 모델, 카드, 그래프)을 비교하는 도구를 P0에 만든다.
4. **smoke:** C# 웹 smoke(`MainWindow.WebShell*.cs`)의 단계를 Tauri smoke로 옮긴다. 화면 확인은 WebView2 원격 디버깅(CDP)으로 DOM 검사와 스크린샷.
5. **나란히 실행:** 개발 동안에는 별도 식별자(`dev.mightycode.tauri`)와 별도 WebView2 폴더로, 실제 상태는 읽기만. 쓰기는 P0의 저장 코드가 C#과 바이트 단위로 같다고 확인된 뒤에 켠다.

## 4. 단계 (Windows 먼저)
| 단계 | 내용 | 완료 기준 |
|---|---|---|
| W0 골격 *(완료 2026-11-13)* | Tauri 앱 + 페이지 연결 + 상태 읽기(쓰기 없음) + 미구현 함수 목록 + smoke | 실제 상태로 작업 공간·창 목록이 보임 |
| W1 상태·플랫폼 *(완료 2026-11-14)* | 상태 저장(원자적 저장, .bak, 디바운스), 정규화, 이름 바꾸기 이전, 프로세스 계층(Job Object, PATHEXT, .cmd), 로케일·토큰, 델타 엔진, 셸 모델 전부 | 상태가 C#과 바이트 단위로 같게 저장; 셸·델타 비교 도구 통과 |
| W2 Claude 요청 *(완료 2026-11-14)* | 실행 엔진(Claude), 스트림 파서, 권한·질문·계획 카드, 체크리스트·백그라운드, 첨부, 작성창·대기열·슬래시 명령 | claude-stream-*, claude-plan-mode, pending, queue-run-now 통과; 실제 Claude 요청이 처음부터 끝까지 됨 |
| W3 마이티 흐름 *(완료 2026-11-14)* | 그래프 추적기, 흐름·타임라인·구조·역할 보기, 진행 패널, 알림 대기 | graph-vectors, flow-vectors, agent-notices 통과 |
| W4 창 종류 *(완료 2026-11-14)* | 터미널(ConPTY + xterm), 파일 창·미리보기·변경점, 브라우저 창(자식 웹뷰), HTML 프레임, 작업 지도, 세션 기록·이어하기 | file-diff, work-map 통과; 해당 smoke 통과 |
| W5 다른 에이전트 *(완료 2026-11-14)* | Codex(exec + 승인 채널), Antigravity(설정 바이트 단위 편집), pi, opencode, Hermes, 모델 목록·단계별 모델 | 각 fixture와 acp-graphs 통과 |
| W6 설정·관리 *(완료 2026-11-14)* | 설정 화면 16개 섹션과 대화 상자 14개(NativeNode), 플러그인, CLI 계정·업데이트, 설정 도구, 사용량, 상태 줄, 알림 | setup-catalog, toolkit-hash 통과; 설정 smoke 통과 |
| W7 위임 *(완료 2026-11-15)* | Agent IO MCP, 위임 MCP 서버, 작업 트리·병합, 자식 창 | delegation-modes와 git 시나리오 통과 |
| W8 원격 *(완료 2026-11-15, 실제 휴대폰 확인 남음)* | 휴대폰(릴레이·직접·m1·푸시), 원격 작업 공간 v2, 펫(겹침 창), 화면 공유(지금 꺼져 있음 — 마지막) | relay, push, mobile-*, remote-v2 통과; 실제 휴대폰 연결 |
| W9 업데이트·출시 *(통합 2026-11-16, CI·수동 확인 남음)* | `--update-helper` 손넘김, 업데이트 확인·서명 검증(Ed25519 동작 일치), 설치 프로그램, CI(`native-windows.yml`), 도움말 캡처, 스타일 엔진(동결본 그대로) | 0.4.59 → 0.5 실제 업데이트 성공; 전체 테스트·smoke 통과 |

## 5. Mac (Windows 이후)
- 같은 Rust 코어에 Mac 플랫폼 계층: forkpty, 로그인 셸 환경, Keychain, UserDefaults 읽기, 휴대폰 페어링 파일 4개, 경로.
- 플랫폼마다 다른 모듈은 같은 포트에 다른 구현을 끼운다(`docs/tauri-architecture.md`). **Mac 터미널은 Ghostty를 그대로 쓴다**(네이티브 뷰를 페이지의 NativeSlot 위에 올림, 지금 Swift 코드 재사용). Mac 브라우저는 CEF(지금과 같음) 또는 WKWebView를 설정으로 고른다. 한글 입력 보조도 Swift 모듈로 유지한다. 메뉴 막대·딥 링크·알림은 Tauri API로.
- 0.4.x Mac → 0.5: 번들 id `dev.mightycode.native`, 실행 파일 이름 `MightyCode`, 같은 개발자 서명 유지. CI의 Mac smoke 10종 다시 만들기, 서명·공증.

## 6. 진행 원칙
- C# 앱은 0.5까지 0.4.x로 계속 출시한다. 그 사이 C#에 들어가는 변경은 모두 Rust 이전 목록에 한 줄씩 추가한다(차이가 벌어지지 않게).
- 단계마다 브랜치 → 테스트·smoke → main 병합. 미완성 기능은 Tauri 앱 안에서만 "미구현"으로 남고, C# 앱에는 영향 없음.
- 큰 단계는 여러 에이전트로 나눠 동시에 진행하되, 파일 범위가 겹치지 않게 crate 단위로 나눈다.

## 7. 정해야 할 것
1. ~~Mac 기능 변화~~ → 정함(2026-11-13): Mac은 Ghostty 터미널을 유지하고, 모듈은 플랫폼마다 다르게 끼운다(`docs/tauri-architecture.md`).
2. 화면 공유(지금 꺼져 있음)를 0.5에 넣을지, 0.5 이후로 미룰지.
3. 0.4.x → 0.5 손넘김을 Rust exe에 직접 넣을지(추천), 0.4.x 다리 버전을 먼저 낼지.

## 8. 진행 기록

각 단계: 구현 → 테스트(C# 테스트 이전 + 공통 기대값) → 독립 코드 리뷰 → 지적 수정 → 커밋·push → Tauri CI(Windows·Mac) 통과.
정하지 않은 것의 기본값(2026-11-14): 업데이트 손넘김은 Rust exe에 직접, 화면 공유는 W8 맨 끝, 상태 저장은 개발 중 꺼 두고 W9에서 기본으로 켬.
W6 기본값(2026-11-14, `.omc/research/tauri/w6-plan.md`): 자동 업데이트·자동 로그인과 PC를 바꾸는 동작(로그아웃, 로그인, CLI 업데이트, 플러그인·도구 설치, HKCU·`~/.claude/settings.json`·omc/Ouroboros 설정 쓰기)은 W9 전까지 `--allow-state-write`가 있을 때만; smoke에서는 언제나 꺼 둠.
W7 기본값(2026-11-15, `.omc/research/tauri/w7-plan.md` D1–D11): 기준은 0.4.59와 `fix/csharp-from-tauri-review`의 위임 수정(PaneWorktree·미러 닫기는 `origin/main` 병합 뒤 W7-E); `--allow-state-write` 없이 위임은 읽기만(트리·보류 항목 표시, 서버 안 붙임, Ctrl+Enter는 대기열, 카드 동작은 확인 모드라고만 말함); Agent IO는 C#처럼 켜 둠(설정 파일 안 씀, smoke에서는 꺼 둠); Mac 전송은 트레잇과 줄 단위 framing만, Unix 소켓은 Mac 단계(시나리오·파이프 테스트는 Windows 전용); git 실행기는 `mighty-run::git`; 카드 병합 커밋 서명은 강화된 C#과 같게(서명 프로그램 없음); 페이지에 없는 작업 트리 에이전트 메뉴 대신 메뉴만 상태의 Ctrl+Enter는 대기열; smoke의 `phone*` 키는 W8 전까지 폰 라우터가 부를 함수를 직접 호출; CI에서 `scripts/delegation-record.py`로 S1·S2 기록 판정; RenameMigration의 작업 트리 부분은 W9; `webOpenChoose`는 C#이 구현하고 README 미구현 목록이 카드 메서드를 놓친 것을 적음.

| 단계 | 커밋 | 리뷰 | CI |
|---|---|---|---|
| W0 골격 | 4df8f04c | — | Windows 통과 |
| W1 상태·플랫폼 | 79d0c26a, 00fd9e4c, d3115cdd | W1-C 리뷰 7건 수정 | 43450108376 Windows·Mac 통과 |
| W2 Claude 요청 (W2-A 실행 엔진, W2-B 작성창) | 2f6aafa2(W2-B 병합), 8677d8a9(W2-A 병합) | 리뷰 대기 | — |
| W3 마이티 흐름 *(완료 2026-11-14)* | 1740632b(병합) | 리뷰 대기 | — |
| W2·W3 호스트 연결 (실행 서비스·작성창·카드·마이티 push와 메서드, 미뤄 둔 검사, smoke 실행 단계, 실제 Claude 요청 1회) | 2b99e71a 이후 이 브랜치의 연결 커밋들 | 리뷰 대기 | — |
| W2 Claude 요청 + W3 마이티 흐름 | 8677d8a9, 1740632b, b6091c22, 44f33041, f2b57b05 | 리뷰 3건(실행 엔진 1 HIGH·4 MEDIUM, 그래프 1 HIGH·1 MEDIUM, 작성창·호스트 1 HIGH·2 MEDIUM-HIGH·3 MEDIUM) 모두 수정 | 43491106177 Windows·Mac 통과 |
| W4 창 종류 (W4-A 터미널, W4-B 파일 창·참조·HTML 프레임·그림 보기, W4-C 세션 기록·이어하기·작업 지도, W4-D 프레임 메서드·대화 상자·작업 공간 만들기·브라우저 창 전부) | f20a9ffc, 6b066f63, 1e029a10, b0bce5bc(병합), 6ff9e43e(이어하기 처음부터 끝까지), 883d4ca8(파일 창 연결), e7bf10f7(레이아웃 동작 하나로), 6b368436(글자 설정·터미널, /rename), 7395a72a(smoke), 062fc6ef·55edac0c(리뷰 수정 병합), 22144f65(Mac 테스트) | 리뷰 4건(터미널, 파일, 기록·작업 지도, 브라우저·대화 상자; HIGH 2건: git 서명 프로그램 실행, iCCP 압축 폭탄) 지적 50건 모두 수정(C#에도 있던 4건은 C# 브랜치 `fix/csharp-from-tauri-review`) | 43553036970 Windows·Mac 통과 |
| W5 다른 에이전트 (W5-A Codex 승인·세션 기록·pi, W5-B opencode·Hermes, W5-C Antigravity, W5-D 모델 목록·단계별 모델·보내기 대상) | 198c5b36(네 부분 병합 w5-final), 65ea00ec 등(리뷰 수정 병합), 7c36a9d8(feat/tauri 병합), cbe4300d(CI 기계의 8.3 임시 경로·Mac 테스트) | 리뷰 5건(W5-A, W5-B, W5-C 정확성·보안, W5-D; HIGH 3건: 대기열에서 지운 요청 실행, Antigravity 규칙의 UNC 경로 NTLM 유출과 경로 별칭 우회) 지적 모두 수정. 보안 리뷰의 공통 규칙(H1·H2·M1·M2·L3·L4)과 B4·B11·B12는 C#·Swift에도 같게(`fix/csharp-from-tauri-review` 5c328851, C#·Mac CI 통과) | 43570264704 Windows·Mac 통과 |
| W6 설정·관리 (W6-A 네이티브 폼 틀·설정 화면·대화 상자·알림, W6-B 설치 화면·도구 모음·상태 줄, W6-C CLI 계정·로그인 복구·사용량, W6-D CLI 업데이트·자동 업데이트·플러그인) | 9c4e5b6f(골격), 8b3d33a8(A0 병합), aaf39df1(A1), a60313d8·d08c88a9(리뷰 수정 병합), e0fa1cb7(통합: localeKeyLeakScan, C# 화면 캡처 비교와 기준 화면), 1324891b(통합 리뷰 수정), 1201942f(CI 기계의 관리자 SID) | 리뷰 4건(A 0 HIGH·7 MEDIUM, B 0 HIGH·4 MEDIUM, C HIGH 2건: 계정 잠금·상태 잠금 교착, 로그인 터미널 다시 안 열림, D 0 HIGH·3 MEDIUM) 모두 수정, 통합 리뷰 1건(HIGH 1: Mac에서 기준 화면 테스트) 수정. 남은 것: 알림 센터 클릭(COM 활성화, W9), 모델 추가 칸 Enter(페이지 필요). 도구 모음 스킬 폴더 `D:evil` 탈출은 C#에도 있어 C# 목록에 추가 | 43602933975 Windows·Mac 통과 |
| W7 위임 (W7-A 위임 핵심·상태 파일·조정자와 도구 6개, W7-B git·작업 트리·병합·정리, W7-C MCP 서버 두 개·Agent IO 파이프·open_url, W7-D 호스트 연결·두 번째 보내기·자식 카드·childAction·S1~S3 시나리오·위임 smoke) | b41cd1f5(골격), e0cd24d5(D 1단계), 6ac4dd49·e05182ed·fbb8bbd0(A), ec24cfd3·fede4157·08095681·fa6bb6d9(B), c988a832·f847a321·4f5c4a20·59d4c7ee(C), 5f259677·f93f4c49(D 2단계·마무리), 3e17d0db·a9e27255(CI: 테스트의 경쟁 상태, Mac 임시 경로 링크, CI 기계의 8.3 임시 경로) | 리뷰 5건(A 0 HIGH·3 MEDIUM, B 0 HIGH·4 MEDIUM, C 보안 0 HIGH·LOW 6, D 0 HIGH·5 MEDIUM, D 마무리 검증 1건) 모두 수정. 자식 작업 트리 폴더가 정션으로 바뀐 경우 폐기가 다른 작업 트리를 지우는 문제는 C#·Swift에도 있어 함께 수정(`fix/csharp-from-tauri-review` 71b4aa1a, C#·Mac CI 통과). 프로세스 종료 대기 스레드 스택 오버플로(W1 코드) 수정. MCP 출력은 C# exe와 바이트 단위로 같음. 남은 것: W7-E 창별 작업 트리(main 0.4.61), RenameMigration 작업 트리 절반(W9), 읽기 전용 실행의 보류 행 바로 실행 숨김(지금은 검증 모드 안내) | 43640316022 Windows·Mac 통과 |
| W8 원격 (W8-A 릴레이·암호·신원·푸시, W8-B 휴대폰 m1 라우터, W8-C 직접 연결·Tailscale·방화벽, W8-D 원격 작업 공간 v2, W8-E 펫, W8-F 화면 공유 순수 코드) | 1e9c5a76(골격), fd5ef53e·01687235·f6ca2aa2·1a4ed297(A), aae64c5b·bc58df6d·f00f0778(B), 3e8d13a4·2d20bb97·aa1c6085(C), 29d00943·a8612393·fff490bb·e74097ce(D), f8e6fe04·816a396d·20c45fce(E), 197c0a39·59527a30(F), f3358ba9~178e9656(통합: D·C 정리, 잠금을 쥔 채 부르는 곳 정리와 lint, 간헐 실패 3개, 같은 창 두 실행 겹침 방지, CI의 언어·Mac 소켓 버퍼) | 리뷰 7건(A·B·C·D·F 보안, E 정확성, 통합) 모두 수정. HIGH 3건: 펫 창 스레드가 닫혀도 끝나지 않음, 펫 말풍선과 셸 시계의 교착, 화면 공유 zstd 프레임 크기 필드 없음(휴대폰이 거부). 방화벽 규칙이 다른 실행 파일을 가리켜도 최신으로 보이는 문제는 C#에도 있어 함께 수정(`fix/csharp-from-tauri-review` e86cbbe4, CI 통과). MCP·휴대폰 응답·원격 요청 머리·푸시·페어링 URL은 C#과 바이트 단위로 같음. 남은 것: 실제 휴대폰·두 PC 상호 운용 확인(`.omc/research/tauri/w8-manual-checklist.md`), 화면 공유의 화면 캡처·입력·WebRTC(D11, 0.5 이후), 스타일·앱 업데이트 부분(W9), W7-E·W8-G(main 0.4.61 창별 작업 트리·원격 미러) | 43685995153 Windows·Mac 통과 |
| W9 업데이트·출시 (W9-A 업데이트 핵심, W9-B 손넘김·Windows 업데이트 플랫폼, W9-C 앱 안의 업데이트·휴대폰, W9-D 스타일, W9-E 출력 꼬리·에이전트 정의·Ouroboros, W9-F 쓰기 정책·식별자·RenameMigration·알림 활성화, W9-G 패키징·CI·리허설, W9-H 도움말 캡처·perf, W9-I 창별 작업 트리, W9-J 원격 미러) | `w9-int`(열 부분 병합과 통합 수정) | 부분별 리뷰(보안 리뷰 HIGH 3건: 프로세스 그룹의 Ctrl+C 무시, 휴대폰 전송의 스타일 우회, 그 밖) 지적 모두 수정, 통합에서 남은 항목(빈 `ALLOWED_UNIMPLEMENTED`, 펫 창 종료 경쟁, 부하 때 깨지던 검사들) 수정 | 리허설 43720095190: 0.4.59·0.4.61에서 실제 업데이트 성공(smoke·로그 읽기 두 검사는 w9-int에서 수정) |
