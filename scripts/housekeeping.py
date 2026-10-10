#!/usr/bin/env python3
"""数据卫生 CLI：默认只读体检，``--apply`` 才真正清理。

背景：项目不使用数据库，``data/`` 是唯一真相源，因此保留策略只能落在文件侧
（孤儿图片、合并后残留的片段音频、长年卡在中间态的会话/记录）。

用法::

    # 只读体检（默认，绝不改数据；环境变量未设置时即为该模式）
    uv run python scripts/housekeeping.py

    # 只读体检 + JSON 报告（便于接入监控/工单）
    uv run python scripts/housekeeping.py --json

    # 显式执行清理（等价于 ATT_HOUSEKEEPING_APPLY=1）
    uv run python scripts/housekeeping.py --apply

    # 自定义阈值/数据目录
    uv run python scripts/housekeeping.py --data D:\\att-data --stuck-hours 24 --json

安全约定：
- 不带 ``--apply`` 且未设置 ``ATT_HOUSEKEEPING_APPLY=1`` 时，本脚本只扫描、只打印；
- 所有删除/改状态动作都在 ``app.repositories.housekeeping`` 内做目录包含性校验；
- 想强制只读（即使环境变量为 1）可显式加 ``--dry-run``。
"""
from __future__ import annotations

import argparse
import asyncio
import json
import sys
from pathlib import Path

# 允许 `python scripts/housekeeping.py` 直接运行（无需先安装包）
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.repositories.housekeeping import (  # noqa: E402  （必须在 sys.path 调整之后导入）
    ALL_CATEGORIES,
    APPLY_ENV,
    CATEGORY_LABELS,
    DEFAULT_DETAIL_LIMIT,
    DEFAULT_ORPHAN_MIN_AGE_HOURS,
    DEFAULT_STUCK_AFTER_HOURS,
    Housekeeping,
)


def build_parser() -> argparse.ArgumentParser:
    """构造命令行参数。"""

    parser = argparse.ArgumentParser(
        prog="housekeeping",
        description="ATT 数据卫生体检/清理（默认只读，--apply 才动数据）",
    )
    parser.add_argument(
        "--data",
        default=None,
        help="data 目录（默认取 app.config.settings.data_path）",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--apply",
        action="store_true",
        help="真正执行清理（删除孤儿图片/残留片段、把卡死状态标记为失败）",
    )
    mode.add_argument(
        "--dry-run",
        action="store_true",
        help="强制只读，即使设置了 %s=1" % APPLY_ENV,
    )
    parser.add_argument("--json", action="store_true", help="以 JSON 输出完整报告")
    parser.add_argument(
        "--stuck-hours",
        type=float,
        default=DEFAULT_STUCK_AFTER_HOURS,
        help="中间态超过该小时数即判定为卡死（默认 %(default)s）",
    )
    parser.add_argument(
        "--orphan-min-age-hours",
        type=float,
        default=DEFAULT_ORPHAN_MIN_AGE_HOURS,
        help="孤儿图片的最小年龄，避免与图片捕获流程竞态（默认 %(default)s）",
    )
    parser.add_argument(
        "--detail-limit",
        type=int,
        default=DEFAULT_DETAIL_LIMIT,
        help="报告里每类问题最多列出的明细条数（默认 %(default)s）",
    )
    return parser


def _format_bytes(size: int) -> str:
    """人类可读的体积。"""

    value = float(size)
    for unit in ("B", "KB", "MB", "GB"):
        if value < 1024 or unit == "GB":
            return f"{value:.1f} {unit}" if unit != "B" else f"{int(value)} B"
        value /= 1024
    return f"{value:.1f} GB"


def _print_human_report(report, *, detail_limit: int) -> None:
    """打印人类可读报告（中文）。"""

    mode = "只读体检" if report.dry_run else "清理执行"
    print(f"ATT 数据卫生 —— {mode}")
    print(f"  data 目录：{report.data_path}")
    print(f"  生成时间：{report.generated_at}")
    print(
        "  扫描量："
        f"记录 {report.scanned.get('records', 0)} / "
        f"会话 {report.scanned.get('sessions', 0)} / "
        f"剪贴板条目 {report.scanned.get('clipboard_items', 0)} / "
        f"剪贴板图片 {report.scanned.get('clipboard_images', 0)}"
    )
    print("  发现的问题：")
    for category in ALL_CATEGORIES:
        count = report.count_of(category)
        size = report.total_bytes(category)
        size_text = f"，占用 {_format_bytes(size)}" if size else ""
        applied = report.applied.get(category, 0)
        applied_text = f"（已处理 {applied}）" if not report.dry_run else ""
        print(f"    - {CATEGORY_LABELS[category]}：{count} 个{size_text}{applied_text}")

    details = [item for item in report.findings]
    if details:
        print(f"  明细（每类最多 {detail_limit} 条）：")
        shown = 0
        per_category: dict = {}
        for item in details:
            per_category.setdefault(item.category, 0)
            if per_category[item.category] >= detail_limit:
                continue
            per_category[item.category] += 1
            shown += 1
            display = item.to_dict(report.data_path)
            print(f"    [{CATEGORY_LABELS[item.category]}] {display['path']} —— {item.detail}")
        if shown < len(details):
            print(f"    …… 其余 {len(details) - shown} 条见 --json 报告")

    if report.errors:
        print(f"  跳过/异常 {len(report.errors)} 条：")
        for message in report.errors[:detail_limit]:
            print(f"    ! {message}")

    if report.dry_run:
        print("  提示：只读模式，未删除任何文件、未改任何状态。")
        print("        执行清理请加 --apply（或设置 %s=1）。" % APPLY_ENV)
    else:
        total = sum(report.applied.values())
        print(f"  完成：实际处理 {total} 项。")


def main(argv=None) -> int:
    """CLI 入口。"""

    args = build_parser().parse_args(argv)

    data_path = Path(args.data) if args.data else None

    if args.apply:
        dry_run = False
    elif args.dry_run:
        dry_run = True
    else:
        # 未显式指定：由环境变量裁决（默认只读）
        dry_run = None

    housekeeping = Housekeeping(
        data_path,
        stuck_after_hours=args.stuck_hours,
        orphan_min_age_hours=args.orphan_min_age_hours,
    )

    if not args.json and dry_run is False:
        print(
            f"警告：即将对 {housekeeping.data_path} 执行破坏性清理，"
            "孤儿图片与残留片段音频会被删除、卡死状态会被标记为失败。",
            file=sys.stderr,
        )

    if dry_run is False:
        # 清理必须走事件循环：状态改写要用异步资源锁 + 原子写
        report = asyncio.run(housekeeping.run(dry_run=False))
    else:
        # 只读体检无需事件循环，同步扫描即可（scan() 天然只读）
        report = housekeeping.scan()

    if args.json:
        print(json.dumps(report.to_dict(detail_limit=args.detail_limit), ensure_ascii=False, indent=2))
    else:
        _print_human_report(report, detail_limit=args.detail_limit)

    if not report.dry_run and report.errors:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
