#!/usr/bin/env python3
"""派卡守卫：让多条开发线并行而不抢同一块代码。

调度模型：一个「窗口」= 一条 lane，窗口之间并行；窗口内可同时挂多张卡，
只要 `touches` 互不重叠（同 crate 的不同子目录也算不重叠）。防冲突靠
**路径边界 + 原子认领**，不靠限制并发数；合并统一**串行**。

任务单 `tasks/<task>.json`：

    {
      "worktree": "/abs/path/worktrees/<task>",   # 每卡独立 worktree
      "base": "origin/main",                      # 本卡基线
      "slot": 0,                                  # 编译车道 0/1/2
      "lane": "core",                             # 所属窗口
      "touches": ["crates/agent-core/src/loop"],  # 硬边界
      "depends_on": ["other-task"]                # 必须已 merged
    }

状态在 `state/<task>.json`：
claimed|running|verifying|merging|done|merged|failed

用法：lanes / status / check <task> / claim <task> / release <task> /
      verify <task> / merge <task> / merged <task>
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path.home() / "lengsuo-agent-dev"
TASKS = ROOT / "tasks"
STATE = ROOT / "state"
LOCK = ROOT / ".guard.lock"

# 全局共享资源：任何普通卡都不许改，否则必然互相覆盖。
# 需要改时单独开「基建卡」，touches 只写这些路径，并串行执行。
SHARED_PATHS = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".github",
    "scripts",
)

# 建议 lane 划分：按 crate 依赖关系分组，使跨 lane 重叠概率最低。
LANES = {
    "core": ["agent-core", "agent-tasks"],
    "proto": ["agent-acp", "agent-gateway"],
    "exec": ["agent-tools", "agent-tools-exec", "agent-pty", "agent-execpolicy"],
    "cap": ["agent-mcp", "agent-skills", "agent-prompts"],
    "env": ["agent-memory", "agent-sandbox", "credential-ref", "policy-engine", "lengsuo-shell"],
}

RUNNING_STATES = ("claimed", "running", "verifying")


class Lock:
    """跨进程排他锁：让"检查 + 占位"成为原子动作，避免两个窗口同时通过检查。"""

    def __enter__(self):
        self._fh = open(LOCK, "w")
        try:
            import fcntl

            fcntl.flock(self._fh, fcntl.LOCK_EX)
        except ImportError:
            # Windows 本地调试用；47（Linux）上恒走 flock。
            pass
        return self

    def __exit__(self, *exc):
        try:
            import fcntl

            fcntl.flock(self._fh, fcntl.LOCK_UN)
        except ImportError:
            pass
        self._fh.close()
        return False


def normalize(path: str) -> str:
    return path.strip().strip("/").replace("\\", "/")


def overlaps(a: str, b: str) -> bool:
    """按**路径段**判断重叠。

    不能用 startswith：`crates/agent-tools` 与 `crates/agent-tools-exec`
    字符串前缀相同，却是两个独立 crate。
    """
    a, b = normalize(a), normalize(b)
    if not a or not b:
        return False
    return a == b or a.startswith(b + "/") or b.startswith(a + "/")


def load_dir(path: Path) -> dict[str, dict]:
    out: dict[str, dict] = {}
    if not path.is_dir():
        return out
    for f in sorted(path.glob("*.json")):
        if f.stem.endswith("-settings"):
            continue
        try:
            out[f.stem] = json.loads(f.read_text())
        except (OSError, ValueError) as e:
            print(f"✗ 读取失败 {f}: {e}", file=sys.stderr)
    return out


def load_tasks() -> dict[str, dict]:
    return load_dir(TASKS)


def load_states() -> dict[str, dict]:
    return load_dir(STATE)


def write_state(task: str, **fields) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    prev = load_states().get(task, {})
    prev.update(fields)
    (STATE / f"{task}.json").write_text(
        json.dumps(prev, ensure_ascii=False, indent=2)
    )


def fail(msg: str, details: list[str] | None = None) -> int:
    print(f"✗ {msg}")
    for line in details or []:
        print(f"    {line}")
    return 1


def collect_errors(task: str, spec: dict, tasks: dict, states: dict) -> list[str]:
    """纯检查：返回所有阻止派卡的理由（空列表 = 可以开）。"""
    errs: list[str] = []
    lane = spec.get("lane")
    touches = [normalize(t) for t in spec.get("touches", [])]
    wt = spec.get("worktree")

    if not lane:
        errs.append("缺少 lane（决定它属于哪个窗口）")
    if not touches:
        errs.append("缺少 touches（必须声明允许改哪些路径）")
    if not wt:
        errs.append("缺少 worktree（每卡必须有独立工作区）")
    if lane and lane not in LANES:
        print(f"  · 提示：lane `{lane}` 不在建议划分内，按自定义处理")

    # 共享路径必须独占
    hits = [t for t in touches for s in SHARED_PATHS if overlaps(t, s)]
    if hits and len(touches) > len(hits):
        errs.append(
            f"共享路径必须独占：{hits}；改 Cargo.toml/scripts 等请单开基建卡"
        )

    # worktree 不得复用
    for name, other in tasks.items():
        if name != task and other.get("worktree") == wt:
            errs.append(f"worktree 与 {name} 相同，每卡必须独立")

    # 路径不得与在跑的卡重叠（跨 lane 的核心约束）
    for name, other in tasks.items():
        if name == task:
            continue
        st = states.get(name, {}).get("state")
        if st not in RUNNING_STATES:
            continue
        for a in touches:
            for b in other.get("touches", []):
                if overlaps(a, b):
                    errs.append(
                        f"与在跑的 {name}（lane={other.get('lane')}）冲突："
                        f"{a} ⟷ {normalize(b)}"
                    )

    # 依赖必须已 merged（不是 done：没合进 main 就开工，会基于旧基线改同一处）
    for dep in spec.get("depends_on", []):
        if dep not in tasks:
            errs.append(f"依赖 {dep} 没有对应任务单")
            continue
        st = states.get(dep, {}).get("state")
        if st != "merged":
            errs.append(f"依赖 {dep} 尚未 merged（当前={st or '未派卡'}）")

    return errs


def cmd_check(task: str) -> int:
    tasks, states = load_tasks(), load_states()
    if task not in tasks:
        return fail(f"找不到任务单 tasks/{task}.json")
    errs = collect_errors(task, tasks[task], tasks, states)
    if errs:
        return fail(f"{task} 不能派卡：", errs)
    print(f"✓ {task} 可以派卡（lane={tasks[task].get('lane')}）")
    return 0


def cmd_claim(task: str) -> int:
    """原子认领：检查与占位在同一把锁内完成。"""
    with Lock():
        tasks, states = load_tasks(), load_states()
        if task not in tasks:
            return fail(f"找不到任务单 tasks/{task}.json")

        cur = states.get(task, {}).get("state")
        if cur in RUNNING_STATES:
            return fail(f"{task} 已被认领（state={cur}, owner={states[task].get('owner')}）")

        errs = collect_errors(task, tasks[task], tasks, states)
        if errs:
            return fail(f"{task} 不能派卡：", errs)

        write_state(
            task,
            state="claimed",
            owner=os.environ.get("USER", "unknown"),
            claimed_at=time.time(),
            lane=tasks[task].get("lane"),
        )

    print(f"✓ {task} 已认领（lane={tasks[task].get('lane')}）")
    return 0


def cmd_release(task: str) -> int:
    with Lock():
        if task not in load_tasks():
            return fail(f"找不到任务单 tasks/{task}.json")
        write_state(task, state="pending", released_at=time.time())
    print(f"✓ {task} 已释放")
    return 0


def cmd_verify(task: str) -> int:
    """校验改动是否越界：覆盖已提交、未提交、未跟踪三类。"""
    tasks = load_tasks()
    if task not in tasks:
        return fail(f"找不到任务单 tasks/{task}.json")

    spec = tasks[task]
    wt = Path(spec.get("worktree", ""))
    touches = [normalize(t) for t in spec.get("touches", [])]
    base = spec.get("base", "origin/main")

    if not wt.is_dir():
        return fail(f"worktree 不存在：{wt}")

    def git(*args: str) -> str:
        r = subprocess.run(
            ["git", "-C", str(wt), *args], capture_output=True, text=True
        )
        return r.stdout if r.returncode == 0 else ""

    changed: set[str] = set()
    # 已提交
    for line in git("diff", "--name-only", f"{base}...").splitlines():
        if line.strip():
            changed.add(normalize(line))
    # 未提交（含暂存与工作区）
    for line in git("diff", "--name-only", "HEAD").splitlines():
        if line.strip():
            changed.add(normalize(line))
    # 未跟踪
    for line in git("ls-files", "--others", "--exclude-standard").splitlines():
        if line.strip():
            changed.add(normalize(line))

    outside = sorted(p for p in changed if not any(overlaps(p, t) for t in touches))

    if outside:
        return fail(
            f"{task} 越界改动 {len(outside)}/{len(changed)} 个文件：",
            outside[:20] + (["…"] if len(outside) > 20 else []),
        )

    with Lock():
        write_state(task, state="verifying", verified_at=time.time())
    print(f"✓ {task} 改动全部在声明范围内（{len(changed)} 个文件）")
    return 0


def cmd_merge(task: str) -> int:
    """串行合流：同一时刻只允许一张卡处于合并流程。"""
    with Lock():
        tasks, states = load_tasks(), load_states()

        if task not in tasks:
            return fail(f"找不到任务单 tasks/{task}.json")
        if states.get(task, {}).get("state") != "verifying":
            return fail(f"{task} 未通过 verify，不能合流")

        others = [
            n
            for n, s in states.items()
            if n != task and s.get("state") in ("verifying", "merging")
        ]
        if others:
            return fail("已有卡在合流流程中，必须串行：", others)

        write_state(task, state="merging", merge_started=time.time())
    print(f"→ {task} 已进入合流；合完请执行：card_guard.py merged {task}")
    return 0


def cmd_merged(task: str) -> int:
    with Lock():
        if task not in load_tasks():
            return fail(f"找不到任务单 tasks/{task}.json")
        write_state(task, state="merged", merged_at=time.time())
    print(f"✓ {task} 标记为已合入")
    return 0


def cmd_status() -> int:
    tasks, states = load_tasks(), load_states()
    if not tasks:
        print("（暂无任务单）")
        return 0

    by_lane: dict[str, list[str]] = {}
    for name, spec in tasks.items():
        by_lane.setdefault(spec.get("lane", "<无 lane>"), []).append(name)

    print(f"{'lane':<9} {'卡':<22} {'状态':<10} touches")
    print("-" * 90)
    for lane in sorted(by_lane):
        for name in sorted(by_lane[lane]):
            st = states.get(name, {}).get("state", "pending")
            mark = {
                "claimed": "◐ 已认领",
                "running": "▶ 跑",
                "verifying": "… 待合",
                "merging": "⇄ 合流中",
                "done": "✓ 完成",
                "merged": "✓ 已合入",
                "failed": "✗ 失败",
            }.get(st, "· 待跑")
            touches = ",".join(tasks[name].get("touches", []))[:38]
            print(f"{lane:<9} {name:<22} {mark:<10} {touches}")

    running = [n for n, s in states.items() if s.get("state") in RUNNING_STATES]
    print(f"\n在跑/占位 {len(running)} 张：{', '.join(running) if running else '无'}")
    print(f"共 {len(tasks)} 张卡，{len(by_lane)} 条 lane")
    return 0


def cmd_lanes() -> int:
    print("建议 lane 划分（按 crate 依赖关系，使跨 lane 重叠概率最低）：\n")
    for lane, crates in LANES.items():
        print(f"  {lane:<8} {', '.join(crates)}")
    print("\n共享路径（普通卡不得改，需单开基建卡独占）：")
    for p in SHARED_PATHS:
        print(f"  {p}")
    print(
        "\n提示：同一 crate 若要并行多张卡，touches 请精确到子目录，"
        "例如 crates/agent-core/src/loop 与 crates/agent-core/src/permission。"
    )
    return 0


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 0

    cmd = sys.argv[1]
    if cmd == "status":
        return cmd_status()
    if cmd == "lanes":
        return cmd_lanes()

    handlers = {
        "check": cmd_check,
        "claim": cmd_claim,
        "release": cmd_release,
        "verify": cmd_verify,
        "merge": cmd_merge,
        "merged": cmd_merged,
    }
    if cmd not in handlers:
        return fail(f"未知命令：{cmd}")
    if len(sys.argv) < 3:
        return fail(f"用法：card_guard.py {cmd} <task>")
    return handlers[cmd](sys.argv[2])


if __name__ == "__main__":
    sys.exit(main())
