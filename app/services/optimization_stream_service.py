"""LLM 流式优化编排：把增量事件、落盘、WebSocket 广播和取消收口放在一处。

调用方有两种：
- HTTP 流式端点：直接消费本生成器并逐行写 NDJSON（broadcast=False，避免同一客户端
  既从 HTTP 又从 WS 收到重复增量）；
- 后台自动优化：`start_background_optimization` 创建任务，通过 WS 广播
  `optimization_started / optimization_delta / optimization_completed / optimization_failed`。

关键约束：
- 任何失败/取消都必须把记录从 optimizing 复位为 transcribed，否则会永久卡状态；
- 失败文案必须脱敏（AGENTS.md §3.4）；
- 流式调用在第一条增量前失败时，允许回退一次非流式 optimize()，兼容不支持 stream 的
  服务商；已经产出增量后不再回退，避免同一段文本重复计费/重复追加。
"""
from __future__ import annotations

import asyncio
from datetime import datetime
from typing import Any, AsyncIterator, Optional

from app.llm.errors import LLMError
from app.llm.optimizer import OptimizationResult, TextOptimizer
from app.llm.security import redact_secrets
from app.records.manager import RecordManager
from app.records.models import RecordStatus
from app.services import records_service
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

# record_id -> 当前正在执行的流式优化任务（手动 HTTP / 后台自动共用）
_active_tasks: dict[str, asyncio.Task] = {}


def _now_iso() -> str:
    return datetime.now().isoformat()


async def _broadcast(event_type: str, data: dict) -> None:
    try:
        await ws_manager.broadcast({
            "type": event_type,
            "data": data,
            "timestamp": _now_iso(),
        })
    except Exception as error:
        logger.warning("推送流式优化事件失败", event=event_type, error=str(error))


def _invalidate(record_manager: RecordManager, record_id: str) -> None:
    records_service.invalidate_record_index(record_manager, record_id)


async def _persist_success(
    record_manager: RecordManager, record_id: str, result: OptimizationResult
) -> None:
    await record_manager.update_record(
        record_id,
        optimized_text=result.optimized_text,
        status=RecordStatus.OPTIMIZED,
    )
    await record_manager.set_error_message(record_id, "")
    _invalidate(record_manager, record_id)


async def _persist_failure(
    record_manager: RecordManager, record_id: str, message: str
) -> None:
    await record_manager.update_record(record_id, status=RecordStatus.TRANSCRIBED)
    await record_manager.set_error_message(record_id, message)
    _invalidate(record_manager, record_id)


async def _reset_after_cancel(record_manager: RecordManager, record_id: str) -> None:
    """取消后的收口。用 shield 防止外层 CancelledError 让清理本身被再次中断。"""

    async def _cleanup() -> None:
        await record_manager.update_record(record_id, status=RecordStatus.TRANSCRIBED)
        await record_manager.set_error_message(record_id, "")
        _invalidate(record_manager, record_id)

    try:
        await asyncio.shield(_cleanup())
    except asyncio.CancelledError:
        # 清理任务已被 shield 放行，继续把取消向上抛
        pass


def _register_current_task(record_id: str) -> Optional[asyncio.Task]:
    task = asyncio.current_task()
    if task is None:
        return None

    previous = _active_tasks.get(record_id)
    if previous is not None and previous is not task and not previous.done():
        # 同一记录不允许两条流同时写；后来的请求取代旧请求
        previous.cancel()

    _active_tasks[record_id] = task
    return task


def _unregister_current_task(record_id: str, task: Optional[asyncio.Task]) -> None:
    if task is not None and _active_tasks.get(record_id) is task:
        _active_tasks.pop(record_id, None)


def is_optimization_active(record_id: str) -> bool:
    task = _active_tasks.get(record_id)
    return bool(task and not task.done())


async def cancel_optimization(record_id: str) -> bool:
    """取消指定记录正在执行的流式优化；没有活动任务返回 False。"""
    task = _active_tasks.get(record_id)
    if task is None or task.done():
        return False

    task.cancel()
    return True


def _done_event(
    record_id: str, result: OptimizationResult, *, degraded: bool = False
) -> dict:
    return {
        "type": "done",
        "record_id": record_id,
        "optimized_text": result.optimized_text,
        "processing_time": getattr(result, "processing_time", 0.0),
        "tokens_used": getattr(result, "tokens_used", 0),
        "truncated": getattr(result, "truncated", False),
        "model_string": getattr(result, "model_string", ""),
        # true 表示流式调用在首条增量前失败，本次结果是回退非流式拿到的
        "degraded": degraded,
    }


async def stream_optimization(
    *,
    record_id: str,
    text: str,
    optimizer: TextOptimizer,
    record_manager: RecordManager,
    preset_id: Optional[str] = None,
    broadcast: bool = True,
    broadcast_terminal: bool = True,
    allow_fallback: bool = True,
) -> AsyncIterator[dict]:
    """逐块产出优化事件，并在结束时落盘。

    Args:
        broadcast: 是否通过 WS 广播 started/delta/terminal（HTTP 端点传 False）。
        broadcast_terminal: 是否广播 completed/failed/cancelled。多段 merge 用 False，
            让 merge 的 HTTP 响应统一负责最终粘贴，避免与 WS 处理重复。
        allow_fallback: 首条增量前失败时是否回退非流式 optimize()。
    """
    task = _register_current_task(record_id)
    completed = False
    reset_done = False
    parts: list[str] = []
    stream_method = getattr(optimizer, "optimize_stream", None)

    async def reset_once(message: Optional[str] = None) -> None:
        nonlocal reset_done
        if reset_done:
            return
        reset_done = True
        await _persist_failure(record_manager, record_id, message or "")

    try:
        await record_manager.update_record(record_id, status=RecordStatus.OPTIMIZING)
        if broadcast:
            await _broadcast("optimization_started", {"record_id": record_id})

        yield {"type": "start", "record_id": record_id, "original_text": text}

        result: Optional[OptimizationResult] = None

        if stream_method is None:
            # 兼容没有流式能力的旧优化器/测试替身：退化为一次完整结果
            result = await optimizer.optimize(text, preset_id=preset_id)
            if result.optimized_text:
                parts.append(result.optimized_text)
                if broadcast:
                    await _broadcast("optimization_delta", {
                        "record_id": record_id,
                        "text": result.optimized_text,
                    })
                yield {"type": "delta", "record_id": record_id, "text": result.optimized_text}
        else:
            async for event in stream_method(text, preset_id=preset_id):
                if event.type == "delta" and event.text:
                    parts.append(event.text)
                    if broadcast:
                        await _broadcast("optimization_delta", {
                            "record_id": record_id,
                            "text": event.text,
                        })
                    yield {"type": "delta", "record_id": record_id, "text": event.text}
                elif event.type == "done":
                    result = event.result

        if result is None:
            raise LLMError("provider_error", "模型没有返回优化结果")
        if not result.optimized_text.strip():
            raise LLMError("provider_error", "模型返回了空结果")

        await _persist_success(record_manager, record_id, result)
        completed = True

        if broadcast and broadcast_terminal:
            await _broadcast("optimization_completed", {
                "record_id": record_id,
                "optimized_text": result.optimized_text,
                "processing_time": getattr(result, "processing_time", 0.0),
                "tokens_used": getattr(result, "tokens_used", 0),
                "truncated": getattr(result, "truncated", False),
            })

        yield _done_event(record_id, result)

    except asyncio.CancelledError:
        await _reset_after_cancel(record_manager, record_id)
        reset_done = True
        if broadcast and broadcast_terminal:
            try:
                await _broadcast("optimization_cancelled", {
                    "record_id": record_id,
                    "raw_text": text,
                })
            except asyncio.CancelledError:
                # 任务已被取消时广播可能再次被中断；状态已复位，允许直接上抛
                pass
        logger.info("流式优化已取消", record_id=record_id)
        raise

    except Exception as error:
        safe_error = redact_secrets(error)

        # 首条增量前失败：允许回退一次非流式调用。已经产出增量后不回退，
        # 否则用户会先看到半段、再看到整段重复文本。
        if allow_fallback and not parts and stream_method is not None:
            try:
                logger.info("流式优化在首条增量前失败，回退非流式调用", record_id=record_id, error=safe_error)
                fallback_result = await optimizer.optimize(text, preset_id=preset_id)
                if fallback_result.optimized_text.strip():
                    if broadcast:
                        await _broadcast("optimization_delta", {
                            "record_id": record_id,
                            "text": fallback_result.optimized_text,
                        })
                    yield {
                        "type": "delta",
                        "record_id": record_id,
                        "text": fallback_result.optimized_text,
                    }
                    await _persist_success(record_manager, record_id, fallback_result)
                    completed = True
                    if broadcast and broadcast_terminal:
                        await _broadcast("optimization_completed", {
                            "record_id": record_id,
                            "optimized_text": fallback_result.optimized_text,
                            "processing_time": getattr(fallback_result, "processing_time", 0.0),
                            "tokens_used": getattr(fallback_result, "tokens_used", 0),
                            "truncated": getattr(fallback_result, "truncated", False),
                        })
                    yield _done_event(record_id, fallback_result, degraded=True)
                    return
                safe_error = "模型返回了空结果"
            except Exception as fallback_error:
                safe_error = redact_secrets(fallback_error)

        await reset_once(safe_error)
        reset_done = True
        if broadcast and broadcast_terminal:
            await _broadcast("optimization_failed", {
                "record_id": record_id,
                "error": safe_error,
                "status": RecordStatus.TRANSCRIBED.value,
                "text": text,
            })
        logger.warning("流式优化失败", record_id=record_id, error=safe_error)
        yield {
            "type": "error",
            "record_id": record_id,
            "error": safe_error,
            "status": RecordStatus.TRANSCRIBED.value,
            "raw_text": text,
        }

    finally:
        if not completed and not reset_done:
            # 客户端提前关闭 HTTP 流（没有走 CancelledError）时也要复位状态
            try:
                await reset_once("流式优化连接已关闭")
            except Exception as error:
                logger.warning("流式优化关闭后复位状态失败", record_id=record_id, error=str(error))
        _unregister_current_task(record_id, task)


async def _consume(events: AsyncIterator[dict]) -> None:
    async for _event in events:
        pass


async def run_optimization(**kwargs) -> None:
    """在当前任务里消费完整条流（单段转写后台任务用，测试也更好等待）。"""
    await _consume(stream_optimization(**kwargs))


def start_background_optimization(
    *,
    record_id: str,
    text: str,
    optimizer: TextOptimizer,
    record_manager: RecordManager,
    preset_id: Optional[str] = None,
    broadcast_terminal: bool = True,
) -> asyncio.Task:
    """创建后台流式优化任务；调用方负责持有/忽略返回的任务。"""
    task = asyncio.create_task(_consume(stream_optimization(
        record_id=record_id,
        text=text,
        optimizer=optimizer,
        record_manager=record_manager,
        preset_id=preset_id,
        broadcast=True,
        broadcast_terminal=broadcast_terminal,
    )))
    task.add_done_callback(lambda finished: _unregister_current_task(record_id, finished))
    return task
