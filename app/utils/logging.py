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

# 日志文件大小上限与保留份数（约 5MB × 5）
LOG_FILE_MAX_BYTES = 5 * 1024 * 1024
LOG_FILE_BACKUP_COUNT = 5


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


def get_logger(name: str) -> structlog.stdlib.BoundLogger:
    """Get a configured logger instance"""
    return structlog.get_logger(name)
