# v0.6.0 Windows 확인 목록 (MC-35)

v0.6.0 작업은 macOS에서만 만들고 시험했습니다. Mac에서는 모든 크레이트를 `x86_64-pc-windows-msvc` 대상으로 clippy까지만 확인했고, C 의존성은 검사 전용 대역으로 대신했습니다. 아래 항목은 실제 Windows PC에서만 확인할 수 있습니다. 한 줄이 확인 하나입니다.

## 공통
- [ ] Windows CI(`native-windows.yml`, `native-tauri.yml`)의 `cargo clippy --workspace --all-targets -- -D warnings`와 `cargo test --workspace`가 통과한다.
- [ ] 릴리스 빌드(GUI 서브시스템)를 설치한 뒤 앱이 뜨고, 상태가 읽히고 저장되며, 종료가 깨끗하다.

## MC-25 워크트리 포트와 스크립트
- [ ] 큰따옴표가 든 작업, 실행 동작, `worktree.setup`, `worktree.teardown`(예: `echo "a b"`, `npm run dev -- --host "127.0.0.1"`)이 `cmd /d /s /c "<명령>"`으로 따옴표 그대로 실행된다.
- [ ] CRLF로 저장된 `.mighty.yaml`의 여러 줄 스크립트가 한 줄 `&&`로 이어져 실행되고, 중간 줄이 실패하면 거기서 멈춘다.
- [ ] `%MIGHTY_PORT%`, `%MIGHTY_PORT_COUNT%`, `%MIGHTY_WORKTREE_NAME%`, `%MIGHTY_ROOT_PATH%`, `%MIGHTY_WORKTREE_PATH%`가 작업, 실행 동작, setup/teardown과 창의 에이전트에 들어간다(공백, 드라이브 문자가 있는 경로 포함).
- [ ] 작업을 "터미널에서 실행"하면 PowerShell 5.1, PowerShell 7(pwsh), cmd 각각에서 폴더 이동, 변수 설정, 명령 실행이 맞게 된다.
- [ ] PowerShell 터미널에서 큰따옴표로 시작하는 명령(`"C:\Program Files\nodejs\node.exe" x.js`)이 cmd로 그대로 넘어간다.
- [ ] 41000번대 포트 블록 배정이 Hyper-V/WSL 예약 포트 범위를 피하고, 빈 포트 검사가 눈에 띄게 느리지 않다.
- [ ] teardown 시간 제한을 넘으면 cmd와 그 자식(`ping`, `node` 등) 전체가 잡 오브젝트로 끝난다.
- [ ] 실행 동작의 개발 서버 주소 감지와 워크트리 바 표시가 맞다.

## MC-26 터미널 알림과 기다리는 창
- [ ] ConPTY 셸의 OSC 9/777/99와 벨이 창에 점과 테두리를 띄우고, 창이 보이지 않을 때만 OS 알림이 뜬다.
- [ ] Ctrl+Shift+U로 가장 최근 기다리는 창으로 간다.
- [ ] 창의 수신 포트(셸, 에이전트, 실행 동작)가 `GetExtendedTcpTable`로 IPv4/IPv6 모두 읽힌다.

## MC-27 병합 전 점검
- [ ] Windows의 `gh`/`glab`로 PR 상태, CI 점검, 리뷰 스레드가 읽힌다.

## MC-28 갈라내기와 의견
- [ ] Claude 세션 갈라내기에서 대화 기록이 새 워크트리 폴더 이름(`C--Users-...`)으로 복사되고 `--fork-session`이 찾는다.
- [ ] 커밋하지 않은 변경(CRLF 파일 포함)이 새 워크트리로 그대로 옮겨진다.

## MC-29 비교 실행
- [ ] 비교 실행의 각 자식 워크트리가 자기 포트 블록을 받고, 테스트 명령(큰따옴표 포함)이 cmd로 돈다.

## MC-30 덩어리별 받기/버리기
- [ ] CRLF 파일, BOM이 있는 파일, 읽기 전용 속성 파일에서 덩어리 버리기가 줄 끝, 인코딩, 속성을 지킨다.
- [ ] 편집기가 연 파일(공유 위반)을 버리려 하면 실패를 알리고 파일은 그대로다.

## MC-31 Linear, Jira, GitLab
- [ ] 트래커 토큰이 DPAPI 저장소(`remote-keys`)에 들어가고, 앱을 다시 켜도 읽힌다.

## MC-32 예약과 트리거
- [ ] cron/RRULE이 Windows 표준 시간대와 서머타임 전환에서 맞게 울린다.
- [ ] 127.0.0.1 웹훅 수신기가 방화벽 창 없이 열린다.
- [ ] 큰따옴표가 든 상시 지시(standing order) 셸 점검이 cmd로 그대로 실행된다.

## MC-9 제어 API, CLI, 훅
- [ ] `mighty-control-<32hex>` 파이프가 현재 사용자 전용이고, 다른 사용자 계정과 원격 클라이언트는 거부된다.
- [ ] 관리자 권한으로 실행한 앱에 일반 권한 CLI가 붙을 때의 동작(소유자가 Administrators라 거부됨)을 확인하고 안내 문구가 이해할 만하다.
- [ ] `control\token`, `control\endpoint.json`의 ACL이 현재 사용자 하나뿐이다.
- [ ] `MightyCode.exe --cli status`를 콘솔에서 바로 실행했을 때 출력이 보이는지 확인한다(GUI 서브시스템 exe라 리디렉션 없이는 안 보일 수 있음. Windows용 `mighty` 명령은 아직 없음).
- [ ] 상태 훅이 이벤트 값을 환경 변수로 받고(`%MIGHTY_...%`), 시간 제한에 걸리면 자식까지 끝난다.

## MC-3 백그라운드 데몬
- [ ] "앱을 닫아도 작업 계속"을 켜고 앱을 닫으면 데몬(`--daemon`)이 작업 전체를 쥔 채 남고, 다시 켜면 셸 스크롤백과 Claude 실행이 돌아온다.
- [ ] 같은 프로필 데몬을 두 번 띄우면 두 번째는 잠금(공유 없이 연 `daemon.lock`) 때문에 바로 끝난다.
- [ ] 데몬이 죽은 뒤(작업 관리자로 종료) 자식 프로세스가 잡 오브젝트로 함께 끝나고, 다음 시작이 남은 endpoint 때문에 멈추지 않는다.
- [ ] 데몬이 돌고 있는 동안 앱 업데이트가 설치 폴더를 바꿀 수 있다(데몬 작업 폴더는 이제 `%TEMP%`. 실행 중인 `MightyCode.exe` 이미지 때문에 폴더 이름 바꾸기가 막히는지 확인).

## MC-3 창 없는 서버
- [ ] `MightyCode.exe --server`가 창 없이 뜨고, 같은 프로필로 창 있는 시작을 하면 서버가 창을 보여 주고 두 번째 시작은 끝난다.
- [ ] 앱이 비정상 종료해 `endpoint.json`이 남아 있어도 `--server`, `mighty server start`가 "실행 중 아님"으로 읽고 새로 시작한다(이번에 고침).
- [ ] `mighty server stop [--stop-all]`이 프로세스 종료까지 기다린다.
- [ ] 서버 창을 닫으면 숨겨지고, 알림, 휴대폰 페어링 승인, 딥 링크가 창을 다시 연다.
- [ ] "로그인할 때 창 없이 시작"이 `HKCU\...\Run`에 `"<exe>" --server [--profile "<폴더>"]`로 들어가고, 다음 로그인 때 창 없이 뜬다. 끄면 값만 지워진다.
- [ ] 끝이 `\`인 프로필 경로(`--profile D:\work\`)로 Run 값을 만든 뒤 로그인 시작이 같은 프로필을 연다(이번에 고침).
- [ ] 콘솔에서 띄운 서버가 Ctrl+C, Ctrl+Break, 콘솔 창 닫기에서 정상 종료 경로(마지막 저장 포함)를 탄다. 콘솔 닫기 뒤 Windows가 프로세스를 끝내기 전에 저장이 끝나는지 확인한다.
- [ ] 로그오프, 종료 때 창 없는 서버의 상태가 저장된다.

## MC-33 SSH 머신
- [ ] `%SystemRoot%\System32\OpenSSH\ssh.exe`(없으면 PATH의 `ssh.exe`)가 쓰이고, `%USERPROFILE%\.ssh\config`의 Host와 Include가 읽힌다.
- [ ] ControlMaster 없이 창마다 연결이 하나씩 열리고, 연결 테스트가 도달 불가, 인증 거부, 호스트 키 문제를 구별한다.
- [ ] ConPTY 안의 `ssh -tt`로 원격 셸과 tmux가 정상 동작하고(한글, 창 크기 변경 포함) 연결이 끊기면 "다시 연결"이 뜬다.
- [ ] 원격 개발 서버 포트 포워딩(`ssh -N -L`)이 창을 닫을 때 프로세스째 끝난다.
- [ ] 원격 Mac에 Mighty Code 서버 설치: 패키지가 ssh 표준 입력으로 끝까지 올라가고(EOF 전달), SHA-256 확인, 설치, 시작, 페어링 터널이 Windows에서도 된다.
- [ ] `<data>\ssh\machines.json`과 `ssh` 폴더의 ACL이 현재 사용자 하나뿐이다.

## MC-34 샌드박스
- [ ] Windows에서 작업 공간 메뉴의 샌드박스 켜기가 "Windows 미지원"으로 거절되고, 휴대폰에도 샌드박스 배지가 나오지 않는다.
- [ ] 다른 기기에서 옮겨 온 상태에 샌드박스가 켜져 있으면 실행이 시작되지 않고 "이 컴퓨터에서는 아직 샌드박스를 쓸 수 없어…"가 창에 보인다(샌드박스 없이 실행하지 않음).

## 휴대폰 화면 (MC-25, MC-27, MC-29)
- [ ] Windows 호스트에 붙은 휴대폰에서 포트 블록, 실행 동작 칩, 점검 시트, 병합 경고, 비교 카드가 Mac 호스트와 같게 보이고 동작한다.
