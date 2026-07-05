#!/usr/bin/env python3
"""PreToolUse(Bash) 가드: 파괴적 명령 차단."""
import json, re, sys

try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)

cmd = (data.get("tool_input", {}) or {}).get("command", "") or ""

patterns = [
    r"push\s+(-f\b|--force)",          # force push
    r"rm\s+-rf\s+(/|~)(\s|$)",         # rm -rf / 또는 ~
    r"git\s+clean\s+-[a-z]*x",         # 추적 안 된 파일 일괄 삭제
    r"git\s+reset\s+--hard\s+origin",  # 원격으로 강제 리셋
]
for p in patterns:
    if re.search(p, cmd):
        print(f"BLOCKED: 파괴적 명령 차단 ({p}). 필요하면 사람이 직접 실행한다.",
              file=sys.stderr)
        sys.exit(2)
sys.exit(0)
