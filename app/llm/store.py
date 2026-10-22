"""LLMStore：档案 + 模型的持久化存储（data/llm_v2.json）。

硬约束遵守情况：
- 所有写入走 `app/repositories/json_store.write_json_atomic`（同目录 tmp + os.replace），
  禁止原地截断写；
- 读-改-写用 `get_thread_lock` 串行化（与 v1 config_manager 同一套模式）；
- 加载**逐条容错**：单条数据损坏只跳过该条并记录，其余照常加载（修复已知问题 #9：
  v1 是循环里直接构造、一条坏数据让全部配置消失）；
- 首次加载时自动把 v1 `llm_configs.json` 迁移为 v2（见 app/llm/migration.py），
  迁移后 v1 文件改名 .v1.bak 保留回滚锚点。
"""
from __future__ import annotations

import uuid
from datetime import datetime
from pathlib import Path
from typing import List, Optional, Tuple

from pydantic import ValidationError

from app.config import settings
from app.llm.errors import CODE_AUTH, LLMError
from app.llm.migration import migrate_v1_to_v2
from app.llm.models import (
    STATUS_DECRYPT_FAILED,
    LLMStoreData,
    ModelEntry,
    ModelItemCreate,
    ModelParams,
    ModelUpdate,
    ProfileCreate,
    ProfileUpdate,
    ProviderProfile,
    VerifyStatus,
    store_public_dict,
    to_jsonable,
)
from app.llm.registry import V1_PROVIDER_MAP, compose_model_string, get_provider_spec
from app.repositories.json_store import (
    ensure_dict,
    get_thread_lock,
    read_json_lenient,
    write_json_atomic,
)
from app.utils.encryption import get_encryption
from app.utils.logging import get_logger

logger = get_logger(__name__)

STORE_FILE_NAME = "llm_v2.json"
LEGACY_FILE_NAME = "llm_configs.json"
LEGACY_BACKUP_SUFFIX = ".v1.bak"


def _new_id(prefix: str) -> str:
    # 时间戳 + 短随机：避免 v1 纯时间戳 id 在批量创建时碰撞
    return f"{prefix}-{int(datetime.now().timestamp() * 1000)}{uuid.uuid4().hex[:4]}"


def _key_hint(plain_key: str) -> str:
    """生成可安全展示的密钥尾号提示（非敏感）。"""
    key = (plain_key or "").strip()
    if not key:
        return ""
    head = key[:3] if key.startswith("sk-") else ""
    tail = key[-4:] if len(key) >= 8 else "****"
    return f"{head}****{tail}" if head else f"****{tail}"


class LLMStore:
    """档案 + 模型两级配置的唯一真相源。"""

    def __init__(self, data_dir: Optional[str | Path] = None):
        self.data_dir = Path(data_dir or settings.data_path)
        self.file = self.data_dir / STORE_FILE_NAME
        self.legacy_file = self.data_dir / LEGACY_FILE_NAME
        # 读-改-写串行化（同一资源一把可重入线程锁）
        self._lock = get_thread_lock(self.file)
        self._data = LLMStoreData()
        # 因数据损坏被跳过的条目（设置页要提示用户，不能静默吞掉）
        self.skipped_entries: List[str] = []
        self._load_or_migrate()

    # ==================== 加载 / 迁移 ====================

    def _load_or_migrate(self) -> None:
        if self.file.exists():
            self._load_v2()
            return
        if self.legacy_file.exists():
            self._migrate_legacy()
            return
        logger.info("未找到 LLM 配置文件，从空配置开始")

    def _load_v2(self) -> None:
        raw = read_json_lenient(self.file, default=None)
        data = ensure_dict(raw, logger=logger, source=self.file)
        skipped: List[str] = []

        profiles: List[ProviderProfile] = []
        for idx, item in enumerate(data.get("profiles") or []):
            try:
                profiles.append(ProviderProfile(**item))
            except (ValidationError, TypeError) as exc:
                label = (item or {}).get("id") if isinstance(item, dict) else None
                skipped.append(f"profile:{label or idx}")
                logger.warning("跳过损坏的 LLM 档案", index=idx, error=str(exc))

        models: List[ModelEntry] = []
        for idx, item in enumerate(data.get("models") or []):
            try:
                models.append(ModelEntry(**item))
            except (ValidationError, TypeError) as exc:
                label = (item or {}).get("id") if isinstance(item, dict) else None
                skipped.append(f"model:{label or idx}")
                logger.warning("跳过损坏的 LLM 模型", index=idx, error=str(exc))

        self._data = LLMStoreData(
            version=2,
            profiles=profiles,
            models=models,
            default_model_id=data.get("default_model_id"),
            fallback_model_id=data.get("fallback_model_id"),
        )
        self.skipped_entries = skipped
        if skipped:
            logger.warning("LLM 配置存在损坏条目，已跳过", skipped=skipped)
        self._heal_custom_model_strings()
        self._repair_bindings()

    def _heal_custom_model_strings(self) -> None:
        """自愈：custom 档案下未带 openai/ 前缀的模型串补齐前缀（加载期一次性修复）。

        背景（2026-10-22 实测踩坑）：compose_model_string 曾对「含 / 的模型名」原样
        透传，本地网关返回的模型 ID（如 `group/deepseek-v4.1-flash`）被原样存库，
        LiteLLM 把首段 group 误当服务商前缀，报 "LLM Provider NOT provided"。
        custom 档案的正确模型串恒为 `openai/<完整模型ID>`，这里在加载期统一修复。
        """
        changed = False
        profiles = {p.id: p for p in self._data.profiles}
        for entry in self._data.models:
            profile = profiles.get(entry.profile_id)
            if profile is None or profile.provider != "custom":
                continue
            if entry.model.startswith("openai/"):
                continue
            entry.model = f"openai/{entry.model}"
            changed = True
        if changed:
            self._save()
            logger.info("已为 custom 档案的历史模型串补齐 openai/ 前缀", count=sum(1 for _ in self._data.models))

    def _repair_bindings(self) -> None:
        """default/fallback 指向不存在的模型时自动回落并回写。"""
        valid_ids = {m.id for m in self._data.models}
        changed = False
        if self._data.default_model_id not in valid_ids:
            self._data.default_model_id = self._data.models[0].id if self._data.models else None
            changed = True
        if self._data.fallback_model_id is not None and self._data.fallback_model_id not in valid_ids:
            self._data.fallback_model_id = None
            changed = True
        if changed and self._data.models:
            self._save()

    def _migrate_legacy(self) -> None:
        logger.info("检测到 v1 LLM 配置，开始自动迁移", legacy=str(self.legacy_file))
        report = migrate_v1_to_v2(self.data_dir)
        if report is None:
            return
        self._data = LLMStoreData(
            profiles=report.profiles,
            models=report.models,
            default_model_id=report.default_model_id,
        )
        self.skipped_entries = list(report.skipped)
        self._save()
        # 迁移成功后把 v1 文件改名保留（回滚锚点），避免下次启动重复迁移
        backup = self.legacy_file.with_name(LEGACY_FILE_NAME + LEGACY_BACKUP_SUFFIX)
        try:
            self.legacy_file.rename(backup)
        except OSError as exc:
            logger.error("v1 配置改名备份失败（不影响 v2 使用）", error=str(exc))
        logger.info(
            "v1 LLM 配置迁移完成",
            profiles=len(report.profiles),
            models=len(report.models),
            skipped=report.skipped,
        )

    def _save(self) -> None:
        """原子写（调用方负责持有 self._lock 或保证单线程语境）。"""
        write_json_atomic(self.file, to_jsonable(self._data))

    def _save_locked(self) -> None:
        with self._lock:
            self._save()

    # ==================== 档案 ====================

    def list_profiles(self) -> List[ProviderProfile]:
        return list(self._data.profiles)

    def get_profile(self, profile_id: str) -> Optional[ProviderProfile]:
        return next((p for p in self._data.profiles if p.id == profile_id), None)

    def create_profile(self, data: ProfileCreate) -> ProviderProfile:
        spec = get_provider_spec(data.provider)
        if spec is None:
            raise ValueError(f"未知服务商：{data.provider}")
        name = (data.name or "").strip()
        if not name:
            raise ValueError("档案名称不能为空")
        api_base = (data.api_base or "").strip() or None
        if spec.id == "custom" and not api_base:
            raise ValueError("自定义 OpenAI 兼容端点必须填写 API Base")

        with self._lock:
            now = datetime.now().isoformat(timespec="seconds")
            profile = ProviderProfile(
                id=_new_id("prof"),
                provider=data.provider,
                name=name,
                api_key_encrypted=get_encryption().encrypt(data.api_key) if data.api_key else "",
                api_key_hint=_key_hint(data.api_key),
                api_base=api_base,
                extra_headers=dict(data.extra_headers or {}),
                extra_query=dict(data.extra_query or {}),
                status=VerifyStatus(state="unknown", checked_at=now),
            )
            self._data.profiles.append(profile)
            self._save()
        logger.info("创建 LLM 供应商档案", profile_id=profile.id, provider=profile.provider)
        return profile

    def update_profile(self, profile_id: str, data: ProfileUpdate) -> Optional[ProviderProfile]:
        with self._lock:
            profile = self.get_profile(profile_id)
            if profile is None:
                return None
            credentials_changed = False
            if data.name is not None and data.name.strip():
                profile.name = data.name.strip()
            if data.api_key is not None and data.api_key != "":
                profile.api_key_encrypted = get_encryption().encrypt(data.api_key)
                profile.api_key_hint = _key_hint(data.api_key)
                credentials_changed = True
            if data.api_base is not None:
                new_base = data.api_base.strip() or None
                if new_base != profile.api_base:
                    profile.api_base = new_base
                    credentials_changed = True
            if data.extra_headers is not None:
                profile.extra_headers = dict(data.extra_headers)
            if data.extra_query is not None:
                profile.extra_query = dict(data.extra_query)
            if credentials_changed:
                # 凭据变了，旧验证结论不再可信
                profile.status = VerifyStatus(state="unknown", checked_at=datetime.now().isoformat(timespec="seconds"))
            self._save()
        logger.info("更新 LLM 供应商档案", profile_id=profile_id)
        return profile

    def delete_profile(self, profile_id: str) -> str:
        """删除档案并级联其下模型；含默认模型时返回 conflict_default（409）。"""
        with self._lock:
            profile = self.get_profile(profile_id)
            if profile is None:
                return "not_found"
            model_ids = {m.id for m in self._data.models if m.profile_id == profile_id}
            if self._data.default_model_id in model_ids:
                return "conflict_default"
            self._data.models = [m for m in self._data.models if m.profile_id != profile_id]
            if self._data.fallback_model_id in model_ids:
                self._data.fallback_model_id = None
            self._data.profiles = [p for p in self._data.profiles if p.id != profile_id]
            self._save()
        logger.info("删除 LLM 供应商档案", profile_id=profile_id, cascaded_models=len(model_ids))
        return "deleted"

    def set_profile_status(self, profile_id: str, status: VerifyStatus) -> None:
        with self._lock:
            profile = self.get_profile(profile_id)
            if profile is None:
                return
            profile.status = status
            self._save()

    def decrypt_key(self, profile: ProviderProfile) -> str:
        """解密档案密钥。无密钥返回空串；密文损坏抛 LLMError（auth_failed）。"""
        if not profile.api_key_encrypted:
            return ""
        try:
            return get_encryption().decrypt(profile.api_key_encrypted)
        except ValueError as exc:
            logger.error("LLM 密钥解密失败", profile_id=profile.id)
            raise LLMError(
                CODE_AUTH,
                "已保存的 API Key 无法解密（可能更换了设备或加密文件），请重新填写密钥",
            ) from exc

    # ==================== 模型 ====================

    def list_models(self, profile_id: Optional[str] = None) -> List[ModelEntry]:
        models = self._data.models
        if profile_id is not None:
            models = [m for m in models if m.profile_id == profile_id]
        return list(models)

    def get_model(self, model_id: str) -> Optional[ModelEntry]:
        return next((m for m in self._data.models if m.id == model_id), None)

    def create_models(self, items: List[ModelItemCreate]) -> dict:
        """批量创建模型条目。返回 {"created": [...], "skipped": [{"model", "reason"}]}。"""
        created: List[ModelEntry] = []
        skipped: List[dict] = []
        with self._lock:
            for item in items:
                profile = self.get_profile(item.profile_id)
                if profile is None:
                    skipped.append({"model": item.model, "reason": f"档案不存在：{item.profile_id}"})
                    continue
                try:
                    model_string = compose_model_string(profile.provider, item.model, profile.api_base)
                except ValueError as exc:
                    skipped.append({"model": item.model, "reason": str(exc)})
                    continue
                if any(m.profile_id == profile.id and m.model == model_string for m in self._data.models):
                    skipped.append({"model": item.model, "reason": "该档案下已存在同名模型"})
                    continue
                entry = ModelEntry(
                    id=_new_id("mdl"),
                    profile_id=profile.id,
                    model=model_string,
                    display_name=item.display_name.strip() or item.model,
                    params=item.params or ModelParams(),
                )
                self._data.models.append(entry)
                created.append(entry)
                if item.set_default or (not self._data.default_model_id and self._data.models):
                    self._data.default_model_id = entry.id
            if created:
                self._save()
        logger.info("批量创建 LLM 模型", created=len(created), skipped=len(skipped))
        return {"created": created, "skipped": skipped}

    def update_model(self, model_id: str, data: ModelUpdate) -> Optional[ModelEntry]:
        with self._lock:
            entry = self.get_model(model_id)
            if entry is None:
                return None
            if data.display_name is not None:
                entry.display_name = data.display_name.strip() or entry.model
            if data.model is not None:
                profile = self.get_profile(entry.profile_id)
                if profile is None:
                    raise ValueError(f"档案不存在：{entry.profile_id}")
                entry.model = compose_model_string(profile.provider, data.model, profile.api_base)
            if data.params is not None:
                entry.params = data.params
            self._save()
        logger.info("更新 LLM 模型", model_id=model_id)
        return entry

    def delete_model(self, model_id: str) -> str:
        with self._lock:
            entry = self.get_model(model_id)
            if entry is None:
                return "not_found"
            if self._data.default_model_id == model_id:
                return "conflict_default"
            if self._data.fallback_model_id == model_id:
                self._data.fallback_model_id = None
            self._data.models = [m for m in self._data.models if m.id != model_id]
            self._save()
        logger.info("删除 LLM 模型", model_id=model_id)
        return "deleted"

    def set_model_status(self, model_id: str, status: VerifyStatus) -> None:
        with self._lock:
            entry = self.get_model(model_id)
            if entry is None:
                return
            entry.status = status
            self._save()

    def set_default_model(self, model_id: str) -> bool:
        with self._lock:
            if self.get_model(model_id) is None:
                return False
            self._data.default_model_id = model_id
            self._save()
        return True

    def set_fallback_model(self, model_id: Optional[str]) -> bool:
        with self._lock:
            if model_id is not None and self.get_model(model_id) is None:
                return False
            self._data.fallback_model_id = model_id
            self._save()
        return True

    # ==================== 运行时解析 ====================

    def get_default_model(self) -> Optional[ModelEntry]:
        """默认模型；绑定缺失时自动回落到第一个模型（已由 _repair_bindings 修正）。"""
        if self._data.default_model_id:
            return self.get_model(self._data.default_model_id)
        return self._data.models[0] if self._data.models else None

    def get_fallback_model(self) -> Optional[ModelEntry]:
        if self._data.fallback_model_id:
            return self.get_model(self._data.fallback_model_id)
        return None

    def resolve_runtime(
        self, model_id: Optional[str] = None
    ) -> Tuple[ModelEntry, ProviderProfile, str]:
        """解析出可调用三元组（模型、档案、明文密钥）。

        供优化器与验证端点使用；密钥仅存在于内存中。失败抛 LLMError（已分级）。
        """
        entry = self.get_model(model_id) if model_id else self.get_default_model()
        if entry is None:
            raise LLMError("model_not_found", "尚未配置可用的 LLM 模型，请在设置页添加")
        profile = self.get_profile(entry.profile_id)
        if profile is None:
            raise LLMError("model_not_found", f"模型所属的供应商档案不存在：{entry.profile_id}")
        plain_key = self.decrypt_key(profile)
        return entry, profile, plain_key

    # ==================== 视图 ====================

    def overview(self) -> dict:
        return store_public_dict(self._data, self.skipped_entries)


# 模块级单例
_store: Optional[LLMStore] = None


def get_llm_store() -> LLMStore:
    """全局单例。测试里请直接构造 LLMStore(tmp_path)，或 monkeypatch 本函数。"""
    global _store
    if _store is None:
        _store = LLMStore()
    return _store


def reset_llm_store() -> None:
    """测试/迁移工具用：丢弃全局单例（下次访问重建）。"""
    global _store
    _store = None
