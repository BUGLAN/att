"""
Logging configuration using structlog
"""
import logging
import sys
from logging.handlers import RotatingFileHandler
from pathlib import Path

import structlog
from structlog.types import Processor

from app.config import settings
from app.llm.security import redact_secrets

# 日志文件大小上限与保留份数（约 5MB × 5）
LOG_FILE_MAX_BYTES = 5 * 1024 * 1024
LOG_FILE_BACKUP_COUNT = 5


def _redact_value(value):
    """递归地把日志事件里的字符串做密钥脱敏，其它类型原样返回。"""
    if isinstance(value, str):
        return redact_secrets(value)
    if isinstance(value, dict):
        return {key: _redact_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return type(value)(_redact_value(item) for item in value)
    return value


def redact_secrets_processor(logger, method_name, event_dict):
    """兜底脱敏：任何日志（含第三方库）里的密钥都不允许落盘。

    AGENTS.md §3.4 要求「日志与异常文案里不得出现密钥」，但第三方库不受我们控制，
    例如 ``websockets`` 的 DEBUG 日志会把请求头（含 ``X-Api-Key``）原样打印。
    除了压低这些库的日志级别，这里再加一道与来源无关的兜底。
    """
    for key, value in list(event_dict.items()):
        event_dict[key] = _redact_value(value)
    return event_dict


def _force_utf8_stream(stream):
    """把标准流切到 UTF-8，返回可用的流对象。

    历史现象：每次转写都会打印一段 ``--- Logging error ---``，
    根因是**控制台** handler 往 GBK 代码页的 stdout 写入私用区字符（U+E100）时抛
    UnicodeEncodeError。只给文件 handler 指定 encoding 治不了这个根因，
    必须把控制台流本身切成 UTF-8；``errors="backslashreplace"`` 保证即使再出现
    非 UTF-8 可表示的内容也只是转义输出，而不会让日志本身报错。

    非 TextIOWrapper（例如已被重定向成字节流）时原样返回，绝不抛异常。
    """
    try:
        stream.reconfigure(encoding="utf-8", errors="backslashreplace")
        return stream
    except (AttributeError, ValueError, OSError):
        return stream


def setup_logging() -> None:
    """Configure structured logging for the application"""

    # Create logs directory if it doesn't exist
    log_dir = Path("logs")
    log_dir.mkdir(exist_ok=True)

    # 控制台流切 UTF-8（见 _force_utf8_stream 说明）；stderr 同样处理，
    # 否则 logging 打印异常回溯时仍可能因编码失败而静默丢日志。
    console_stream = _force_utf8_stream(sys.stdout)
    _force_utf8_stream(sys.stderr)

    # Shared processors for all handlers
    shared_processors: list[Processor] = [
        structlog.contextvars.merge_contextvars,
        structlog.stdlib.add_logger_name,
        structlog.stdlib.add_log_level,
        structlog.processors.TimeStamper(fmt="iso"),
        structlog.stdlib.PositionalArgumentsFormatter(),
        structlog.processors.StackInfoRenderer(),
        structlog.processors.UnicodeDecoder(),
        # 放在渲染之前：无论日志来自应用还是第三方库，密钥都先被洗掉
        redact_secrets_processor,
    ]

    # Configure structlog
    structlog.configure(
        processors=shared_processors + [
            structlog.stdlib.ProcessorFormatter.wrap_for_formatter,
        ],
        logger_factory=structlog.stdlib.LoggerFactory(),
        wrapper_class=structlog.stdlib.BoundLogger,
        cache_logger_on_first_use=True,
    )

    # Console handler
    console_handler = logging.StreamHandler(console_stream)
    console_handler.setLevel(logging.DEBUG if settings.debug else logging.INFO)
    console_formatter = structlog.stdlib.ProcessorFormatter(
        foreign_pre_chain=shared_processors,
        processors=[
            structlog.dev.ConsoleRenderer(colors=True),
        ],
    )
    console_handler.setFormatter(console_formatter)

    # File handler：带 rotation + 显式 utf-8
    # 旧实现用 FileHandler，logs/app.log 无上限增长（已到数 MB）；
    # 显式 encoding="utf-8" 是为了不依赖平台默认编码（Windows 为 GBK）。
    file_handler = RotatingFileHandler(
        log_dir / "app.log",
        maxBytes=LOG_FILE_MAX_BYTES,
        backupCount=LOG_FILE_BACKUP_COUNT,
        encoding="utf-8",
    )
    file_handler.setLevel(logging.DEBUG)
    file_formatter = structlog.stdlib.ProcessorFormatter(
        foreign_pre_chain=shared_processors,
        processors=[
            structlog.processors.JSONRenderer(),
        ],
    )
    file_handler.setFormatter(file_formatter)

    # Configure root logger
    root_logger = logging.getLogger()
    root_logger.setLevel(logging.DEBUG if settings.debug else logging.INFO)
    root_logger.handlers = [console_handler, file_handler]

    # Reduce noise from third-party libraries
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    # websockets 的 DEBUG 日志会把请求头（含 X-Api-Key 豆包 Access Token）原样打出来，
    # 既泄露凭据又让单次录音产生上百 KB 日志，因此整条链路压到 WARNING
    logging.getLogger("websockets").setLevel(logging.WARNING)
    # openai / anthropic SDK 的 DEBUG 日志会把整个请求体打出来：提示词里嵌着
    # 用户刚转写的原文。项目既有约定是只记长度（text_length / original_length）、
    # 不记内容，因此这些 SDK 的请求体日志一并压掉
    logging.getLogger("openai").setLevel(logging.WARNING)
    logging.getLogger("anthropic").setLevel(logging.WARNING)


def get_logger(name: str) -> structlog.stdlib.BoundLogger:
    """Get a configured logger instance"""
    return structlog.get_logger(name)
