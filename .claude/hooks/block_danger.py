#!/usr/bin/env python3
"""PreToolUse(Bash) 가드:
1) 파괴적 명령 차단
2) GitHub Flow 강제 — main 직접 commit/push/merge 차단
   (가드레일이다: `git switch main && git commit` 같은 복합 명령은 훅 시점
   브랜치로만 판정한다. 완전 봉쇄는 GitHub 브랜치 보호 규칙이 담당.)
차단 시 exit 2 + stderr 메시지(에이전트에게 피드백됨)."""
import json
import re
import subprocess
import sys

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


def block(msg):
    print(f"BLOCKED: {msg}", file=sys.stderr)
    sys.exit(2)


def current_branch():
    """훅 프로세스 cwd 기준 현재 브랜치. 리포 밖이면 None (오차단 방지)."""
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            capture_output=True, text=True, timeout=5,
        )
        return out.stdout.strip() if out.returncode == 0 else None
    except Exception:
        return None


if re.search(r"\bgit\b", cmd):
    # refspec 이 main 을 명시하는 push 는 현재 브랜치와 무관하게 차단
    if re.search(r"\bgit\s+(?:[^&|;]*\s)?push\b[^&|;]*\s(?:[\w./-]+:)?main\b", cmd):
        block("main 으로의 push 금지 — 브랜치를 push 하고 PR 로 머지한다 (GitHub Flow).")
    if current_branch() == "main":
        if re.search(r"\bgit\s+commit\b", cmd):
            block("main 직접 커밋 금지 — 브랜치를 먼저 따라 (feat/*, fix/*, chore/*, docs/*).")
        if re.search(r"\bgit\s+push\b", cmd):
            block("main 에서의 push 금지 — 브랜치에서 작업하고 PR 로 머지한다.")
        if re.search(r"\bgit\s+(merge|rebase)\b", cmd):
            block("main 에서의 merge/rebase 금지 — PR 로만 main 에 합친다.")

sys.exit(0)
