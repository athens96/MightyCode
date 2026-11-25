# MightyCode 기본 아이콘

사용자가 제공한 안경 쓴 너구리 이미지를 편집한 슈퍼히어로 마스코트입니다.
얼굴·안경·노트북·줄무늬 꼬리를 유지하고, 파란 슈트·빨간 망토·M 문양을 적용했습니다.
macOS 26+(Liquid Glass)·iOS·Android·웹에서 둥근 사각형 아이콘 영역을 가장자리까지 가득 채우도록 원형 배지 테두리와 투명 모서리를 제거하고, 네 모서리까지 이어지는 파란 그라데이션 풀블리드 정사각형 원본으로 업데이트했습니다.

- 생성 방식: 내장 이미지 생성 도구로 기존 `mightycode.png` 편집 후 24px 외곽 심리스 여백 보정
- 원본 생성 파일: `mightycode.png`
- 최초 날짜: 2026-09-16 / 풀블리드 정사각형 업데이트: 2026-11-25

## 결과와 적용

- `mightycode.png`: 1024×1024 RGBA 풀블리드 정사각형 원본(네 모서리까지 불투명 파란 배경).
- `MightyCode.icns`: macOS용 16–1024px 표현 10개. Apple macOS HIG 연속 곡률 스쿼클(824/1024 비율, 미세 그림자)로 패키징해 macOS 14–15와 macOS 26+ 모두에서 회색 테두리 없이 사각형을 가득 채운다.
- `MightyCode.ico`: Windows용 16·24·32·48·64·128·256px 표현 7개(타일 전체를 채우는 연속 곡률 둥근 사각형).
- `bash scripts/package-icons.sh`: Swift/CoreGraphics와 iconutil을 사용한 크기·포맷 변환. 원본 그림은 변경하지 않는다.
- `python3 scripts/generate-phone-assets.py`: 모바일(`mobile/assets/images/` — iOS 1024×1024 풀블리드 `icon.png`, Android 적응형 포그라운드/모노크롬, 스쿼클 스플래시·파비콘) 및 `asset-sources.json` 재생성.
- 웹·사이드바·다운로드 페이지 파생 아이콘(`native/windows/web/src/assets/app-icon.png`, `site/assets/icon.png`, `scripts/download-page/icon.png`)도 같은 풀블리드 원본에서 갱신한다.
- macOS 기본 앱 아이콘과 사이드바, Windows EXE·창 아이콘과 브랜드 이미지에 연결했다.

## 최종 생성 프롬프트

```text
Use case: precise-object-edit / logo-brand.
Asset type: final square desktop app icon for MightyCode, 1024x1024 PNG.
Input image 1 is the edit target. Preserve this specific friendly gray raccoon character: big round black glasses, rounded ears, raccoon eye mask, cheerful open-mouth smile, large fluffy striped tail on the right, paws typing on a laptop in the lower left, bold dark cartoon outlines, clean polished flat/vector-like illustration. Preserve the recognizable proportions, centered composition and blue circular technology badge.
Primary change: transform ONLY the raccoon's green hoodie into a Superman-like superhero costume: vivid royal blue fitted superhero suit, bright red flowing cape clearly visible behind the shoulders and curling beside the tail, a simple gold belt where visible, and a bold red-and-gold chest shield with the single capital letter "M" clearly readable for MightyCode. Make the heroic outfit immediately recognizable while keeping the raccoon adorable and still typing on its laptop. Let the chest shield be visible above the paws/laptop.
Icon refinement: retain the blue circular background with very subtle simplified circuit traces. Simplify tiny laptop code marks to a few colorful bars so the icon stays readable when small. Keep generous safe padding around the complete circular badge, cape, ears, laptop and tail, no cropping. The area outside the blue circular badge must be genuinely transparent, not black, not white, not a checkerboard drawing. Crisp anti-aliased edges, strong silhouette, polished professional mascot app icon. No additional characters, no extra limbs, no title text, no watermark. Only lettering is "M" on the chest. Preserve the original face, spectacles, friendly expression, laptop and tail identity. Output a single finished icon, no mockup or icon grid.
```
