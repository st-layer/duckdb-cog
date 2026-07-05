#!/usr/bin/env bash
# Stop hook: `just check` 통과 전 턴 종료를 차단한다.
# (Claude Code는 8회 연속 차단 시 훅을 무시하고 종료 — 무한루프 방지 내장)
set -u
input=$(cat)

# stop_hook_active=true 면 이미 이 훅 때문에 계속 중인 상태 → 재차단 루프 방지
active=$(printf '%s' "$input" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("stop_hook_active", False))
except Exception: print(False)')
if [ "$active" = "True" ]; then
  # 이 실행에서도 check가 통과해야 통과시킨다 (아래로 계속)
  :
fi

cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0

out=$(just check 2>&1)
status=$?
if [ $status -ne 0 ]; then
  {
    echo "BLOCKED: 'just check' 실패 — 완료 선언 전에 아래를 고쳐라."
    echo "---- 마지막 출력 60줄 ----"
    printf '%s\n' "$out" | tail -n 60
  } >&2
  exit 2
fi
exit 0
