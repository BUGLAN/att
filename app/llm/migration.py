"""v1 → v2 一次性迁移：把旧 `llm_configs.json` 拆成 档案 + 模型 两级结构。

聚合规则（docs/LLM配置模块重设计.md §9.1）：
- 按「provider + 解密后 key + api_base」聚合为 ProviderProfile（同 key 多配置只存一份密文）；
- 每条 v1 配置生成一个 ModelEntry，模型名按 V1_PROVIDER_MAP 补全 LiteLLM 前缀；
- v1 的 default（default_config_id / is_default）映射到对应模型条目 id；
- 单条失败：跳过并记录原因，其余照常（不重蹈 v1「全有或全无」的覆辙）；
- 解密失败的条目：保留原密文并标记 decrypt_failed，允许用户重填 key 修复。
"""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from app.llm.models import (
    STATUS_DECRYPT_FAILED,
    ModelEntry,
    ModelParams,
    ProviderProfile,
    VerifyStatus,
)
from app.llm.registry import V1_PROVIDER_MAP, compose_model_string, get_provider_spec
from app.repositories.json_store import ensure_dict, read_json_lenient
from app.utils.encryption import get_encryption
from app.utils.logging import get_logger

logger = get_logger(__name__)

# v1 时代豆包档案的默认端点（当时 PROVIDER_INFO 的默认值），迁移 custom 且无 base 时兜底
_DOUBAO_LEGACY_BASE = "https://ark.cn-beijing.volces.com/api/v3"


@dataclass
class MigrationReport:
    profiles: List[ProviderProfile] = field(default_factory=list)
    models: List[ModelEntry] = field(default_factory=list)
    default_model_id: Optional[str] = None
    skipped: List[str] = field(default_factory=list)


def _migrate_provider_id(v1_provider: str) -> str:
    """v1 枚举值 → v2 注册表 id；未知值保守映射为 custom（openai 兼容兜底）。"""
    return V1_PROVIDER_MAP.get((v1_provider or "").strip().lower(), "custom")


def migrate_v1_to_v2(data_dir: str | Path) -> Optional[MigrationReport]:
    """执行迁移；v1 文件不存在时返回 None（改由调用方决定后续动作）。"""
    legacy_file = Path(data_dir) / "llm_configs.json"
    if not legacy_file.exists():
        return None

    raw = read_json_lenient(legacy_file, default=None)
    data = ensure_dict(raw, logger=logger, source=legacy_file)
    entries = data.get("configs") or []
    report = MigrationReport()
    if not isinstance(entries, list):
        logger.warning("v1 配置文件结构异常，按空配置迁移")
        return report

    encryption = get_encryption()
    # (provider, key_marker, api_base) → profile
    profile_index: dict[tuple, ProviderProfile] = {}
    v1_default_id = data.get("default_config_id")
    migrated_default_v1_id: Optional[str] = None

    for idx, item in enumerate(entries):
        if not isinstance(item, dict):
            report.skipped.append(f"entry:{idx}:非对象")
            continue
        config_id = str(item.get("id") or f"entry:{idx}")
        try:
            provider_v1 = str(item.get("provider") or "")
            provider_v2 = _migrate_provider_id(provider_v1)
            model_name = str(item.get("model") or "").strip()
            if not model_name:
                report.skipped.append(f"{config_id}:缺少模型名")
                continue
            spec = get_provider_spec(provider_v2)
            api_base = (str(item.get("api_base") or "").strip() or None)
            # 旧 custom/未知 provider 且没有端点的：无法组装可用模型串，跳过并提示
            if api_base is None and provider_v1.strip().lower() == "doubao":
                api_base = _DOUBAO_LEGACY_BASE
            try:
                model_string = compose_model_string(provider_v2, model_name, api_base)
            except ValueError:
                report.skipped.append(f"{config_id}:custom 端点缺失，无法确定 API Base")
                continue

            encrypted = str(item.get("api_key_encrypted") or "")
            plain_key = ""
            decrypt_failed = False
            if encrypted:
                try:
                    plain_key = encryption.decrypt(encrypted)
                except ValueError:
                    decrypt_failed = True

            # 聚合档案：解密失败的用密文本身做 marker（同密文归同一档案，便于统一重填）
            marker = plain_key if not decrypt_failed else f"enc:{encrypted}"
            key_tuple = (provider_v2, marker, api_base)
            profile = profile_index.get(key_tuple)
            if profile is None:
                legacy_name = str(item.get("name") or "").strip()
                spec = spec or get_provider_spec("custom")
                profile = ProviderProfile(
                    id=f"prof-m{idx}{len(profile_index)}",
                    provider=provider_v2,
                    name=legacy_name or spec.name,
                    # 解密失败的保留原密文，用户重填 key 后即可修复
                    api_key_encrypted=encrypted if decrypt_failed else (encryption.encrypt(plain_key) if plain_key else ""),
                    api_key_hint="",
                    api_base=api_base,
                    status=VerifyStatus(
                        state=STATUS_DECRYPT_FAILED if decrypt_failed else "unknown",
                        checked_at=datetime.now().isoformat(timespec="seconds"),
                        error_code="auth_failed" if decrypt_failed else None,
                        error="已保存的密钥无法解密，请重新填写" if decrypt_failed else None,
                    ),
                )
                profile_index[key_tuple] = profile
                report.profiles.append(profile)

            params = ModelParams(
                temperature=float(item.get("temperature", 0.3) or 0.3),
                max_tokens=int(item.get("max_tokens", 4096) or 4096),
            )
            entry = ModelEntry(
                id=f"mdl-m{idx}",
                profile_id=profile.id,
                model=model_string,
                display_name=legacy_name or model_name,
                params=params,
            )
            report.models.append(entry)

            was_default = bool(item.get("is_default")) or (v1_default_id and config_id == v1_default_id)
            if was_default and report.default_model_id is None:
                report.default_model_id = entry.id
                migrated_default_v1_id = config_id
        except (TypeError, ValueError) as exc:
            # 单条数据非法：跳过，绝不让一条坏数据拖垮全部配置（v1 的教训）
            report.skipped.append(f"{config_id}:字段非法({exc})")
            continue

    logger.info(
        "v1 配置迁移解析完成",
        profiles=len(report.profiles),
        models=len(report.models),
        skipped=report.skipped,
        default_v1=migrated_default_v1_id,
    )
    return report
