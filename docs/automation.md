# 예약과 트리거 (자동 실행)

> MC-32, 0.6.0. 순수 부분은 `native/tauri/crates/mighty-core/src/automation/`(cron·RRULE 계산, 정의, 계획, 실행 기록, 트리거 상태, 실행 단계), `.mighty.yaml` 키는 `crates/mighty-delegation/src/mighty_yaml.rs`, 앱 쪽은 `native/tauri/src-tauri/src/automation/`(타이머, 실행, 트리거 감시, 웹훅 리스너, 파일)과 `src-tauri/src/settings/jobs.rs`(설정 상자). MC-9의 이벤트 버스·훅·웹훅, MC-25의 `.mighty.yaml` 신뢰 승인, MC-31의 Linear 연결, MC-11의 사용량 장부와 하루 예산, MC-29의 실행 비용 추정을 그대로 쓴다.

설정 › Mighty Pet과 알림 › **예약과 트리거** 상자에서 워크스페이스마다 예약(cron/RRULE)과 이벤트 트리거를 두고, 정한 때에 에이전트 작업을 시작한다. 반복해서 손으로 요청하던 일(의존성 점검, 깨진 테스트 정리, 들어온 PR 검토)을 맡긴다.

## 앱이 닫혀 있을 때

**예약과 트리거는 앱이 열려 있을 때만 실행한다.** 앱을 닫으면 타이머와 감시가 함께 멈춘다.

- MC-3 백그라운드 데몬(앱을 닫아도 작업 계속)을 켜 두어도, 데몬은 이미 실행 중인 에이전트 실행과 터미널만 이어 간다. 데몬에는 실행 엔진이 없어(`--headless`도 아직 같다) 새 실행을 시작할 수 없으므로, 데몬이 타이머를 맡지 않는다.
- 앱이 닫혀 있던 동안 놓친 예약은 다음에 앱을 열 때 처리한다: `catchUp: once`(기본)는 **가장 최근에 놓친 한 번만** 실행하고, `catchUp: skip`은 실행하지 않고 "건너뜀" 기록만 남긴다. 컴퓨터가 잠들어 있던 동안 놓친 예약도 깨어날 때 같은 규칙을 따른다(2분 넘게 늦으면 놓친 것으로 본다).
- 그래서 "매일 9시" 예약은 9시에 앱이 열려 있으면 그때, 아니면 그날 처음 앱을 열 때 한 번 돈다. 앱을 닫아 둔 상태에서 도는 예약은 후속(데몬이 실행 엔진을 갖출 때)이다.

## 예약

- **cron**: 5칸(`분 시 일 월 요일`). 각 칸은 `*`, 숫자, `a-b`, `*/n`, `a-b/n`, 쉼표 목록; 월·요일은 이름(`JAN`, `MON`)도 되고 7도 일요일이다. 일과 요일이 둘 다 정해지면 둘 중 하나만 맞아도 실행한다(cron 규칙). `@hourly`, `@daily`, `@weekly`, `@monthly`도 읽는다.
- **RRULE 일부**: `FREQ=DAILY|WEEKLY|HOURLY`, `BYHOUR`, `BYMINUTE`, `BYDAY`(`MO,TU,…`), 앞에 `RRULE:`가 있어도 된다. 시작 시각(DTSTART)이 없으므로 `BYMINUTE` 기본은 0, DAILY·WEEKLY의 `BYHOUR` 기본은 0이고, WEEKLY는 `BYDAY`가 있어야 한다. `COUNT`, `UNTIL`, `INTERVAL`(1 제외)은 받지 않는다.
- **시간대**: 이 컴퓨터의 시간대. 서머타임으로 건너뛴 시각(예: 02:30)은 바뀐 직후 첫 시각에 한 번, 두 번 오는 시각(예: 01:30)은 처음에 한 번만 실행한다. 31일 예약은 31일이 있는 달에만, 2월 29일은 윤년에만 돈다. 5년 안에 오지 않는 줄(`0 0 30 2 *`)은 다음 실행이 없다.
- **타이머 하나**: 스케줄러는 다음 실행까지 잠든다(폴링하지 않는다). 설정이 바뀌거나 승인하거나 컴퓨터가 깨어나면 바로 다시 계산하고, `.mighty.yaml`을 앱 밖에서 고친 것을 읽으려고 늦어도 30분마다 한 번 깨어난다.

## 트리거

| 종류 | 확인 방법 | 중복 거르기 |
|---|---|---|
| GitHub `pr_opened` | `gh api repos/<repo>/pulls?state=open&sort=created…` | PR 번호 |
| GitHub `review_requested` | `gh api repos/<repo>/notifications?participating=true`, reason이 `review_requested` | 알림 id와 갱신 시각 |
| GitHub `ci_failed` | `gh api repos/<repo>/actions/runs?status=failure` | 워크플로 실행 id |
| Linear | MC-31 Linear 연결로 최근 이슈 50개를 읽어 고른 상태로 **들어온** 이슈 | 이슈별 이전 상태 |
| 웹훅 | `127.0.0.1:<포트>/hooks/<id>`로 온 서명된 POST | 전달 id(없으면 본문 해시) |
| 상시 점검(standing order) | 셸 명령을 N분마다 워크스페이스 폴더에서 실행(60초 제한) | 출력(종료 코드 포함)의 해시 |

- 로컬 앱은 GitHub 웹훅을 받을 수 없으므로 `gh api`(사용자의 로그인)로 본다. **GitHub·Linear 트리거가 있는 워크스페이스만**, 가장 잦아도 5분에 한 번 본다. 바뀐 것이 없으면 5→10→20→30분, 실패(오프라인, 로그아웃)하면 10→20→40→60분으로 간격을 늘리고, 새 것이 오면 5분으로 돌아간다. GitHub는 지난 `ETag`를 `If-None-Match`로 보내 304(사용량 한도를 쓰지 않음)를 받는다.
- 감시의 첫 확인은 지금 있는 것을 배우기만 하고 실행하지 않는다. 한 번 확인에 새 이벤트는 최대 3개만 실행하고 나머지는 본 것으로 기억한다(트리거마다 최근 1,000개).
- 상시 점검은 출력이 처음과 달라질 때만 에이전트를 깨우고, 하루 `dailyCap`번(기본 5)까지만 깨운다. 상한에 닿으면 그 변화는 다음 날로 미룬다.
- 트리거의 프롬프트 뒤에는 이벤트(제목, 링크, 점검 출력이나 웹훅 본문의 앞 4,000자)를 `<event source="mighty-automation">…</event>`로 감싸 붙인다. 바깥에서 온 글이므로 에이전트에게 데이터로 보이게 한다.

### 웹훅 리스너

- 기본 꺼짐. 켜면 `127.0.0.1`(루프백)에만 묶고, 루프백이 아닌 연결은 받지 않는다. 포트는 1024 이상(기본 47823). 동시에 8개 연결까지.
- 트리거마다 주소 `http://127.0.0.1:<포트>/hooks/<트리거 키 SHA-256 앞 16자>`와 비밀 키(64 hex)가 있다. 비밀 키는 처음 필요할 때 만들어 OS 키체인(`automation.webhook.<id>`)에만 둔다. 상자의 **비밀 키 복사**는 가능하면 클립보드 기록에서 빠지는 방식으로 복사한다.
- 요청은 `POST`, `Content-Length` 필수(청크 전송 거부), 본문 256KB 이하(넘으면 읽기 전에 413), 헤더 16KB 이하, 10초 안에 다 와야 한다. `X-Mighty-Signature-256: sha256=<본문의 HMAC-SHA256 hex>`(GitHub의 `X-Hub-Signature-256`도 됨)를 상수 시간으로 비교하고, 틀리면 401. 같은 `X-Mighty-Delivery`(또는 `X-GitHub-Delivery`)는 200 "duplicate"로 답하고 버린다. `X-Mighty-Event`/`X-GitHub-Event`는 기록의 제목이 된다.
- 바깥(GitHub, 다른 서비스)에서 받으려면 사용자가 직접 ngrok, Cloudflare Tunnel 같은 터널을 앞에 둔다. 앱은 터널을 열지 않는다.

## 실행

- 정의의 공통 필드: `prompt`, `agent`(기본 `claude`), `model`, `worktree`(true면 새 창을 자기 워크트리에서), `mode`(`new` 새 창 / `heartbeat` 이름이 `pane`인 기존 창을 깨워 같은 대화로 보냄; 그 창이 없으면 그 이름으로 한 번 만든다), `enabled`.
- 새 창은 예약 이름을 제목으로 고정해 연다(자동 제목이 바꾸지 않음). 프롬프트는 휴대폰·`mighty agent send`와 같은 작성기 경로로 보내므로, 바쁜 창이면 대기열에 들어가고 그 창의 스타일과 "보낼 대상" 설정이 적용된다.
- **하루 예산**: 사용량 장부의 오늘 사용액 + 이 에이전트(와 모델)의 최근 30일 평균 실행 비용(MC-29 추정, 기록이 없으면 $1)이 하루 예산을 넘으면 시작하지 않고 "건너뜀"과 이유를 기록한다.
- **알림**: 자동 실행한 창이 승인이나 질문을 기다리면 OS 알림과 휴대폰 푸시(permission/question 종류)로 바로 알린다. 나머지(완료, 실패, 건너뜀)는 실행 기록과 상자의 "오늘 자동화" 요약에 모은다.
- **이벤트 버스**: 실행마다 `automation.fired`, `automation.skipped`, `automation.blocked`, `automation.finished` 이벤트(이름, 종류, 기록 id, 상태, 비용, 결과 링크; 프롬프트와 이벤트 본문은 싣지 않음)를 MC-9 버스에 올린다. `mighty events --type automation.finished`로 받을 수 있고, 자동화 상자의 "알릴 상태"에서 **자동화 실행 결과**를 켜면(기본 꺼짐) 웹훅과 상태 명령이 끝남·건너뜀을 따른다. 상태 명령은 `MIGHTY_EVENT=automation.finished|automation.skipped`, `MIGHTY_AUTOMATION_NAME`, `MIGHTY_AUTOMATION_KIND`, `MIGHTY_AUTOMATION_URL`, `MIGHTY_STATE`를 받는다. `.mighty.local.yaml`의 `hooks.on`에도 `automation`을 쓸 수 있다.

## 실행 기록

- `<data>/automation/history.json`: 실행마다 무엇이(이름, 종류, 출처 app/yaml), 언제(실행·예정 시각), 어느 창에서, 상태(실행 중, 승인·질문 대기, 완료, 실패, 멈춤, 건너뜀), 비용(창의 실행이 끝날 때 장부가 기록한 값, heartbeat처럼 여러 번이면 합), 결과 링크(창의 마지막 글 40개에서 찾은 마지막 GitHub PR 또는 GitLab MR 주소), 오류, 다시 실행한 원래 기록.
- 최대 300개, 512KB. 넘으면 끝난 기록부터 오래된 순으로 지운다. 이유는 300자, 오류는 500자에서 자른다.
- **다시 실행**: 그 기록의 예약·트리거를 지금 다시 실행한다(트리거는 기록에 남은 이벤트 제목과 링크로). 정의가 없어졌거나 꺼졌거나 승인되지 않았으면 할 수 없다. **이어서 하기**: 설정을 닫고 그 창을 연다. **지금 실행**: 예약 시각과 상관없이 한 번 실행한다.

## 정의 위치와 신뢰

- **앱 설정**: `<data>/automation/settings.json`, 워크스페이스 id별 `schedules`, `triggers`와 리스너 설정. 사용자가 직접 넣은 것이므로 승인 없이 실행한다. 상자의 폼으로 더하고 고친다(워크스페이스마다 각 32개까지).
- **`.mighty.yaml` / `.mighty.local.yaml`**: `schedules:`, `triggers:`(이름이 키). 다른 키처럼 정규화한 설정이 내용 해시에 들어가므로 프롬프트나 점검 명령이 한 글자만 바뀌어도 다시 승인해야 한다. 승인 전에는 상자에 "승인 필요"와 **승인** 단추가 나오고, 읽기 전용이다. 개인 파일의 같은 이름은 공유 파일의 것을 대신한다. 두 키가 없는 파일은 해시가 그대로다.
- 상태는 `<data>/automation/state.json`: 예약별 마지막 실행 시각과 처음 본 시각, 트리거별 본 id, ETag, 상시 점검 출력 해시와 오늘 깨운 횟수, Linear 이슈 상태. 없어진 정의의 상태는 지운다. 처음 본 예약은 그때부터 세므로 새로 넣은 예약이 과거 것을 따라잡지 않고, 꺼 두었던 예약은 켠 때부터 센다.

```yaml
schedules:
  deps-check:
    cron: "0 9 * * 1-5"
    prompt: 의존성을 점검하고 필요하면 PR을 여세요.
    worktree: true
    catchUp: once
triggers:
  fix-ci:
    github: [ci_failed]
    prompt: CI 실패를 고치세요.
  flaky-tests:
    standing: npm test 2>&1 | tail -5
    everyMinutes: 30
    dailyCap: 3
    prompt: 테스트 결과가 바뀌었습니다.
```

## 한계(0.6.0)

- 앱이 닫혀 있으면 실행하지 않는다(위). 데몬이 타이머를 맡는 것과 데몬 안에서의 헤드리스 실행은 후속이다.
- 큐에 들어간 heartbeat 프롬프트는 그 창의 다음 실행이 시작될 때부터 기록이 따른다. 창을 사람이 같이 쓰면 비용과 상태가 섞일 수 있다.
- GitHub `review_requested`는 사용자의 알림에 기대므로, 알림을 끈 저장소에서는 오지 않는다. GitLab·Jira 트리거, Slack/메신저 멘션 트리거(MC-1)는 없다.
- 결과 링크는 창의 글에서 찾은 주소다. 에이전트가 PR 주소를 출력하지 않으면 비어 있다.
- 실제 GitHub·Linear·외부 웹훅으로는 확인하지 않았다(검사는 모두 픽스처와 루프백).

## 검사

- `cargo test -p mighty-core --lib automation`: cron·RRULE(필드 형식, 일·요일 OR, 월말·윤년, 서머타임 건너뜀·겹침, 로컬 시간대), 놓친 예약(once·skip·유예·꺼짐·오래된 기록), 정의 검사와 JSON 모양, 실행 기록(개수·크기 상한, 파일 왕복, 요약, 결과 링크), 실행 단계(예산 건너뜀, 다시 실행 대상), 트리거 상태(중복, 첫 확인, 백오프, ETag, 상시 점검 변화와 하루 상한, Linear 상태 진입, 이벤트 프롬프트).
- `cargo test -p mighty-core --lib control`: `automation.*` 이벤트의 JSON·필터와 상태 명령 변수.
- `cargo test -p mighty-delegation --test mighty_yaml`: `schedules`/`triggers` 읽기·검사·병합·해시(바뀌면 다시 승인).
- `cargo test -p mighty-run --test automation`: 가짜 시계로 계획을 돌리고 실제 실행 엔진과 가짜 CLI로 실행 — 정시 실행, 끝까지 따라간 기록(비용, PR 링크), 저장한 상태로 다시 시작해 한 번만 따라잡기, 예산 초과 건너뜀(창을 만들지 않음), 다시 실행.
- `cargo test -p mightycode automation`: 파일 왕복, HMAC(RFC 4231), 리스너(서명 맞음·틀림·없음, 중복 전달, 256KB 초과, POST 아님, 루프백만), `gh api -i` 출력과 304, GitHub 이벤트 읽기.
