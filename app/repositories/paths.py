"""路径安全工具：标识符合法性、目录包含性校验。

审计报告 §4.1 的两个已复现缺陷都源于「把外部输入直接当路径用」：
- 任意文件读取：``metadata.image_path`` 为绝对路径 → 图片接口返回该文件内容；
- 任意 ``*.json`` 越界删除：``DELETE /api/records/..%5C..%5Ctmp%5Cprobe`` 真的把文件删掉了。

本模块提供两个纯函数作为统一闸门：ID 只允许单层安全文件名，文件路径必须在指定目录内。
"""

from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Union

PathLike = Union[str, Path]

# ID 中不允许出现任何路径分隔符、NUL 或单独的点段
_UNSAFE_IDENTIFIER_PATTERN = re.compile(r"[/\\\x00]")


def is_safe_identifier(value: object) -> bool:
    """判断 ID 是否是「单层安全文件名」。

    拒绝：空值、``.``、``..``、任何含 ``/`` ``\\`` NUL 的字符串、
    以及经 :class:`pathlib.Path` 解析后名字发生变化的值（如 Windows 盘符相对路径 ``C:foo``）。
    """

    if not isinstance(value, str) or not value:
        return False

    if value in {".", ".."} or value.strip() != value:
        return False

    if _UNSAFE_IDENTIFIER_PATTERN.search(value):
        return False

    # 兜底：解析后的文件名必须与原值完全一致，杜绝平台相关的路径语义
    return Path(value).name == value


def is_within_directory(path: PathLike, directory: PathLike) -> bool:
    """判断 ``path`` 经解析后是否位于 ``directory`` 目录**内部**。

    目录本身不算「位于其内部」。解析失败或路径不存在时返回 ``False``（失败即拒绝），
    Windows 下额外做大小写不敏感的退化比较。
    """

    try:
        resolved_path = Path(path).resolve()
        resolved_directory = Path(directory).resolve()
    except (OSError, ValueError):
        return False

    if resolved_path == resolved_directory:
        return False

    try:
        resolved_path.relative_to(resolved_directory)
        return True
    except ValueError:
        pass

    # Windows 文件系统大小写不敏感：对未落盘的路径做一次规范化比较
    if os.name == "nt":
        normalized_path = os.path.normcase(str(resolved_path))
        normalized_directory = os.path.normcase(str(resolved_directory)).rstrip("\\/")
        return normalized_path.startswith(normalized_directory + os.sep)

    return False
