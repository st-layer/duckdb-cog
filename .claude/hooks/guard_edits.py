#!/usr/bin/env python3
"""PreToolUse(Edit/Write) 가드:
1) 구현 중 tests/oracle, tests/sql 수정 차단 (ALLOW_TEST_EDITS=1 로 해제)
2) 제품 크레이트 Cargo.toml 에 GDAL/PROJ/GEOS 계열 의존성 추단 차단 (RFC N4)
차단 시 exit 2 + stderr 메시지(에이전트에게 피드백됨)."""
import json, os, re, sys

try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)

ti = data.get("tool_input", {}) or {}
path = ti.get("file_path", "") or ""
new_text = "".join(
    str(ti.get(k, "") or "") for k in ("content", "new_string", "new_str")
)

norm = path.replace("\\", "/")

# 1) 테스트 동결
frozen = ("/tests/oracle/" in norm or norm.rstrip("/").endswith("/tests/oracle")
          or "/test/sql/" in norm or norm.rstrip("/").endswith("/test/sql"))
if frozen and os.environ.get("ALLOW_TEST_EDITS") != "1":
    print(
        "BLOCKED: tests/oracle, test/sql 은 계약이다 — 구현 태스크에서 수정 금지.\n"
        "테스트가 틀렸다고 판단되면 수정 대신 이유를 보고하고 승인을 받아라.\n"
        "(테스트 '작성' 태스크에서만 ALLOW_TEST_EDITS=1 로 해제)",
        file=sys.stderr,
    )
    sys.exit(2)

# 2) N4: 제품 크레이트에 GDAL 계열 의존성 금지
if norm.endswith("Cargo.toml") and "/tests/" not in norm and "/scripts/" not in norm:
    if re.search(r'^\s*(gdal|gdal-sys|geos|geos-sys|proj|proj-sys|proj4rs?)\s*=',
                 new_text, re.MULTILINE | re.IGNORECASE):
        print(
            "BLOCKED: GDAL/PROJ/GEOS 계열 의존성은 읽기 경로에 금지 (RFC N4).\n"
            "오라클/픽스처 용도면 tests/ 또는 scripts/ 의 Python 쪽에서 써라.\n"
            "정말 필요하다고 판단되면 추가하지 말고 사람 승인을 요청하라.",
            file=sys.stderr,
        )
        sys.exit(2)

sys.exit(0)
