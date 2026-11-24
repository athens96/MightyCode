# Windows의 mighty 명령 (MC-43)

> MC-43, 0.6.x. MC-9의 `mighty` CLI를 Windows에서 Mac과 같게 쓴다: `mighty status`, `mighty agent …`, `mighty server …`. 구현: 명령은 `native/tauri/crates/mighty-cli`(`mighty.exe`), 콘솔 붙이기는 `mighty-platform-windows/src/parent_console.rs`, 연결 규칙의 순수 부분은 `mighty-platform/src/control_access.rs`, 파이프는 `mighty-platform-windows/src/agent_io.rs`, CLI 쪽 판단은 `src-tauri/src/control/client.rs`, PATH는 `src-tauri/src/shell_command.rs`(설정 › 자동화의 단추)와 `native/windows/installer/MightyCode.iss`(설치 작업), 빌드는 `scripts/build-tauri-windows.ps1`. 실기기 확인은 [windows-v060-checklist.md](windows-v060-checklist.md)의 "MC-43".

## 명령: `mighty.cmd`가 아니라 콘솔 exe

패키지에 `bin\mighty.exe`(콘솔 서브시스템, std만 쓰는 작은 프로그램)를 넣는다. 하는 일은 하나다: 설치 폴더의 `MightyCode.exe --cli <받은 인자>`를 같은 콘솔에서 실행하고, 끝 코드를 그대로 돌려준다(`MIGHTY_CODE_APP`이 있으면 그 exe).

`mighty.cmd`를 고르지 않은 까닭:
- **인자가 cmd를 한 번 더 지난다.** 배치의 `%*`는 다시 파싱되므로, PowerShell이 따옴표 없이 넘긴 `&`, `|`, `<`, `>`, `^`(예: `mighty agent send p1 a&b`)가 명령으로 실행된다. `agent send`는 남이 쓴 글(이슈 제목 등)을 그대로 넘기는 일이 잦아 명령 주입이 된다(2024년 "BatBadBut"과 같은 종류).
- Ctrl+C에 "Terminate batch job (Y/N)?"가 끼어들고, 셸마다(cmd, PowerShell, Git Bash) 동작이 조금씩 다르다.

콘솔 exe는 Windows가 나눈 인자를 그대로 받고(Rust의 `Command`가 exe 규칙으로 다시 묶는다), 앱이 끝날 때까지 기다리며, 표준 입력·출력·오류(파이프와 파일 포함)를 손대지 않고 넘긴다. Ctrl+C와 Ctrl+Break는 처리기를 달아 자기는 끝나지 않고 앱의 끝을 기다린다. `SetConsoleCtrlHandler(NULL, TRUE)`는 자식에게 "무시"가 상속돼 `mighty events`를 멈출 수 없게 되므로 쓰지 않는다.

`bin` 폴더에 따로 두는 까닭: 그 폴더만 PATH에 넣으면 `mighty` 하나만 보이고, 앱의 다른 파일(MightyCode.exe, Agents…)은 PATH에 나오지 않는다. 앱의 자체 업데이트는 설치 폴더를 통째로 바꾸므로 `bin\mighty.exe`도 함께 바뀌고, 폴더 경로는 그대로라 PATH 항목도 그대로 맞다.

`mighty <폴더> [요청]`(Mac에서 폴더 열기)은 Windows에 아직 길이 없어(딥 링크 `mightycode://`가 Mac에만 등록됨) 그렇게 말하고 끝 코드 1로 끝난다. `mighty`만 치면 사용법을 보여 준다.

## 설치와 PATH

- **설치 프로그램**: `bin\mighty.exe`가 든 패키지면 "mighty 명령을 PATH에 추가 (이 사용자)" 작업이 생기고 기본으로 켜져 있다. `{app}\bin`을 `HKCU\Environment\Path` 끝에 한 번만 더하고(이미 있으면 그대로, 다른 항목은 손대지 않음), `ChangesEnvironment=yes`로 새 터미널과 탐색기에 알린다. 컴퓨터 전체 PATH(HKLM)는 건드리지 않는다(관리자 권한 없음). 제거하면 그 항목만 뺀다.
- **앱 안**: 설정 › Mighty Pet과 알림 › 자동화 › 명령줄 제어에 상태 한 줄과 **PATH에 추가** 단추가 있다(Windows만). 설치 프로그램을 다시 돌리지 않는 자체 업데이트 사용자를 위한 것이다. 상태는 새 터미널이 받을 PATH(레지스트리의 컴퓨터·사용자 PATH를 펼친 것)로 판단한다. 단추는 사용자 PATH 값을 저장된 그대로(이름, `REG_SZ`/`REG_EXPAND_SZ` 형식) 읽어 끝에 붙이고 `WM_SETTINGCHANGE`("Environment")를 보낸다. 쓰기 정책이 기계를 바꾸지 못하게 한 실행(개발 빌드의 기본 등)에서는 아무것도 쓰지 않는다. Mac은 예전처럼 워크스페이스 메뉴의 "셸 명령 'mighty' 설치"를 쓴다.

## 콘솔 출력 (`MightyCode.exe --cli`)

릴리스 `MightyCode.exe`는 GUI 서브시스템이라 콘솔에서 실행해도 콘솔을 받지 못하고, 리디렉션하지 않은 출력은 사라진다. `--cli` 모드는 다른 일보다 먼저 `AttachConsole(ATTACH_PARENT_PROCESS)`로 부모의 콘솔에 붙고, 표준 핸들 가운데 **쓸 수 없는 것만**(없음, 잘못된 핸들) `CONIN$`/`CONOUT$`로 바꾼다. 파일, 파이프, `nul`, 물려받은 콘솔 핸들은 그대로라 `> out.json`, `| jq`, `< in.txt`, `mighty`가 넘긴 핸들이 지켜진다. Rust의 표준 스트림은 쓸 때마다 핸들을 다시 읽으므로 다른 코드는 바뀌지 않는다. 콘솔에 붙으면 그 콘솔의 Ctrl+C도 받는다.

남는 한계: cmd나 PowerShell에서 `MightyCode.exe --cli status`를 **직접** 치면 셸이 GUI 프로그램을 기다리지 않아 출력이 다음 프롬프트 뒤에 찍히고, `agent send … -`의 표준 입력을 셸과 나눠 읽는다. 그래서 사람은 `mighty`를 쓰고, 직접 호출은 리디렉션하거나(`… > out.json`) 스크립트에서만 쓴다.

## 관리자 권한 앱과 일반 권한 CLI

### 규칙

제어 API의 파이프(`mighty-control-<32 hex>`)는:
1. **이 사용자 계정만**: 소유자와 하나뿐인 접근 항목(파이프 전체 권한 `0x1f019f`)이 토큰의 **사용자** SID다. 예전에는 토큰의 기본 소유자를 썼는데, 관리자 권한 토큰의 기본 소유자는 Administrators 그룹이라 관리자로 띄운 앱의 파이프에는 **다른 관리자 계정**의 관리자 권한 프로세스도 들어올 수 있었다.
2. **앱보다 낮은 무결성 수준은 못 들어온다**: 파이프에 앱의 무결성 수준(일반 Medium, 관리자 권한 High)을 필수 레이블로 달고 쓰기·읽기 모두 막는다(`S:(ML;;NWNR;;;HI)`). 
3. 원격 클라이언트는 거부(`PIPE_REJECT_REMOTE_CLIENTS`), 요청마다 `control\token`의 토큰(그대로).

CLI는 파이프 소유자가 자기 토큰의 사용자인지 확인한다. 그래서:

| 앱 \ CLI | 일반 권한 CLI | 관리자 권한 CLI |
|---|---|---|
| 일반 권한 앱 | 연결 | 연결 (새로 됨) |
| 관리자 권한 앱 | **거부**, 까닭과 방법을 알림 | 연결 |
| 다른 사용자의 앱 | 거부 | 거부 (관리자 권한이어도) |

### 근거

- **일반 권한 CLI → 관리자 권한 앱을 막는 까닭**: 이 연결을 허락하면 그 사용자의 **아무 일반 권한 프로그램**이나 토큰 파일을 읽고(토큰 파일은 사용자 SID에 열려 있다) 관리자 권한 앱의 에이전트에게 명령을 시킬 수 있다. 관리자 권한으로 명령을 실행하게 하는 UAC 우회다. "같은 로그온 세션의 같은 사용자만"으로는 막지 못한다: UAC의 두 연결 토큰(필터된 토큰과 관리자 토큰)은 같은 사용자 SID와 같은 로그온 SID를 가져서(같은 데스크톱에 접근하므로), 사용자나 로그온 SID로 거르는 규칙은 둘을 가르지 못한다. 둘을 가르는 것은 무결성 수준뿐이라, 규칙을 무결성 레이블로 정했다.
- **관리자 권한 CLI → 일반 권한 앱을 허락하는 까닭**: 위에서 아래로 가는 연결이라 CLI는 권한을 얻지 않고, 앱이 받는 것(토큰과 명령 한 줄)은 일반 권한 프로그램도 이미 닿을 수 있는 것이다. 서버는 CLI를 흉내 낼 수 없다(`SECURITY_IDENTIFICATION`). 관리자 권한 터미널을 기본으로 쓰는 사람이 많아 막으면 쓸모가 크게 준다. 이때 CLI가 받는 답은 JSON으로 찍기만 하고 그 내용으로 무언가를 실행하지 않는다.
- **로그온 세션으로 더 좁히지 않은 까닭**: Mac은 같은 uid의 모든 프로세스를 받는다. Windows에서 같은 사용자의 SSH 세션(OpenSSH 서버, 다른 로그온 세션)에서 `mighty status`를 쓰는 일을 막을 이유가 없고, 위에서 말했듯 엘리베이션 구분에는 도움이 되지 않는다.
- **거절 문구**: 앱은 `control\endpoint.json`에 `"elevated": true|false`를 쓴다(파일 접근은 예전처럼 사용자 SID만). 파이프가 연결을 거부했을 때(접근 거부) 이 값이 참이고 CLI는 관리자 권한이 아니면 "관리자 권한으로 연 터미널에서 실행하거나 앱을 일반 권한으로 시작하라"고 말하고 끝 코드 1로 끝난다. 이 값은 문구만 고르고, 막는 것은 파이프의 레이블이다(값이 틀려도 우회되지 않는다). 그 밖의 접근 거부는 "실행 중이지만 이 연결을 거부함"으로 읽는다. 어느 쪽이든 **"실행 중 아님"으로 읽지 않는다**: 그래야 `mighty server start`가 같은 프로필에 앱을 하나 더 띄우지 않고, `server stop`이 성공한 척하지 않는다. 남은 endpoint가 가리키는 파이프가 없을 때(앱이 죽음)는 예전처럼 "실행 중 아님"이다(MC-35).
- 관리자 권한 터미널에서 `mighty server start`를 하면 앱도 관리자 권한으로 뜬다. CLI가 표준 오류에 한 줄로 알린다(그 뒤 일반 권한 터미널의 `mighty`는 닿지 않는다).
- 설정 › 자동화 › 명령줄 제어는 앱이 관리자 권한이면 "mighty 명령은 관리자 권한으로 연 터미널에서만 연결된다"고 보여 준다.

### MC-42 샌드박스와의 관계

MC-42는 **에이전트 파이프**(`mighty-agent-io-…`)에만 로그온 SID 항목과 Low 레이블을 더해 샌드박스 실행(Low 무결성, 쓰기 제한 토큰)의 MCP 서버가 닿게 했다. 제어 파이프는 그 항목을 받지 않았고 이번에도 받지 않는다. 샌드박스 프로세스는 제어 파이프에 두 겹으로 막힌다: (1) 앱 수준(Medium 이상)의 NWNR 레이블, (2) 쓰기 제한 토큰의 제한 SID(워크스페이스 SID, 에이전트 SID, Everyone, 로그온 SID) 가운데 어느 것도 파이프의 항목(사용자 SID)에 없다. 데몬 파이프와 에이전트 파이프의 규칙은 바뀌지 않았다.
