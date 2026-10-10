"""
FastAPI Application Entry Point
Audio-to-Text API with AI-powered optimization
"""
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse

from app.api.routers.llm import router as llm_router
from app.api.routers.prompts import router as prompts_router
from app.api.routers.records import router as records_router
from app.api.routers.sessions import router as sessions_router
from app.api.routers.settings import router as settings_router
from app.api.routes import close_streaming_session_manager, init_processors
from app.audio.processor import AudioProcessor
from app.clipboard.manager import ClipboardManager
from app.clipboard.routes import init_clipboard_manager
from app.clipboard.routes import router as clipboard_router
from app.config import settings
from app.llm.gateway import get_gateway
from app.llm.optimizer import TextOptimizer
from app.records.manager import RecordManager
from app.repositories.housekeeping import run_startup_housekeeping
from app.repositories.writer_lock import acquire_writer_lock, release_writer_lock
from app.services.model_service_manager import model_service_manager
from app.services.qwen_streaming_service_manager import qwen_streaming_service_manager
from app.stt.factory import create_asr_provider, get_asr_provider_config
from app.utils.logging import get_logger, setup_logging

logger = get_logger(__name__)

# Setup logging
setup_logging()


async def maybe_preload_asr_provider(asr_provider) -> None:
    """根据配置预热 ASR provider，但不要让预热失败阻断服务启动。"""
    if not settings.asr_preload_on_startup:
        logger.info(
            "跳过启动时 ASR 预热",
            provider=asr_provider.provider_type,
            model=asr_provider.model_name,
        )
        return

    logger.info(
        "预热 ASR provider",
        provider=asr_provider.provider_type,
        model=asr_provider.model_name,
    )

    try:
        await asr_provider.warmup()
    except Exception as error:
        logger.warning(
            "ASR 预热失败，继续启动服务",
            provider=asr_provider.provider_type,
            model=asr_provider.model_name,
            error=str(error),
            exc_info=True,
        )


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan - startup and shutdown events"""
    logger.info(
        "Starting Audio-to-Text API",
        app_name=settings.app_name,
        debug=settings.debug,
    )

    # 单写者约束：JSON 文件存储不支持多进程同时写入同一份 data/。
    # 冲突时抛中文 RuntimeError，让启动直接失败，而不是静默丢数据。
    acquire_writer_lock(settings.data_path)

    try:
        audio_processor = AudioProcessor()
        asr_provider = create_asr_provider(get_asr_provider_config())
        optimizer = TextOptimizer()
        record_manager = RecordManager(settings.data_path)
        clipboard_manager = ClipboardManager(settings.data_path)

        await record_manager.initialize()
        await clipboard_manager.initialize()

        # 启动期数据卫生扫描：**永远只读**（只发现并写日志，不受 ATT_HOUSEKEEPING_APPLY 影响）。
        # 真正的清理必须显式执行 scripts/housekeeping.py --apply。
        await run_startup_housekeeping(settings.data_path)

        init_processors(audio_processor, asr_provider, optimizer, record_manager)
        init_clipboard_manager(clipboard_manager)
        model_service_manager.configure(asr_provider)

        await maybe_preload_asr_provider(asr_provider)

        logger.info("All processors initialized")
    except BaseException:
        # 启动中途失败也要释放写者锁，避免把锁文件留给下一次启动去接管
        release_writer_lock()
        raise

    try:
        yield
    finally:
        logger.info("Shutting down Audio-to-Text API")
        await close_streaming_session_manager()
        await model_service_manager.shutdown()
        await qwen_streaming_service_manager.shutdown()
        await asr_provider.close()
        await optimizer.close()
        release_writer_lock()


app = FastAPI(
    title=settings.app_name,
    description="""
Audio-to-Text API with AI-powered optimization.

## Features
- Upload audio files (wav, mp3, m4a, flac, ogg, webm)
- Transcribe using faster-whisper (Chinese optimized)
- Optimize text using DeepSeek AI
- Fragment-based processing with merging capability
- Export in multiple formats (TXT, JSON, SRT)

## Workflow
1. Create a session or upload audio directly
2. Audio is automatically transcribed
3. Optionally optimize text with AI
4. Merge and export fragments
    """,
    version="0.1.0",
    lifespan=lifespan,
)

app.add_middleware(
    CORSMiddleware,
    # 显式白名单：Electron 打包后页面是 file://（Origin: null），
    # 开发期是 Vite 的 :8090；其余来源（含任意 https 站点）一律不返回 CORS 头。
    allow_origins=[
        "http://localhost:8090",
        "http://127.0.0.1:8090",
        "null",
    ],
    allow_origin_regex=r"^https?://(localhost|127\.0\.0\.1)(:\d+)?$",
    # 本地无鉴权接口不能带凭证，避免任意网页跨域读取本地数据
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
)

# API 路由按资源逐个挂载（app/api/routes.py 里的聚合 router 仍保留，供测试与旧引用使用）
app.include_router(records_router)
app.include_router(sessions_router)
app.include_router(settings_router)
app.include_router(prompts_router)
app.include_router(llm_router)
app.include_router(clipboard_router)

project_root = Path(__file__).parent.parent
dist_dir = project_root / "dist"
dist_assets_dir = dist_dir / "assets"

if dist_assets_dir.exists():
    from fastapi.staticfiles import StaticFiles
    app.mount("/assets", StaticFiles(directory=str(dist_assets_dir)), name="assets")


@app.get("/")
async def root():
    """Root endpoint - Serve the Vue frontend when available."""
    dist_index = dist_dir / "index.html"
    if dist_index.exists():
        return FileResponse(str(dist_index))

    return {
        "name": settings.app_name,
        "version": "0.1.0",
        "docs": "/docs",
        "health": "/api/health",
    }


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "app.main:app",
        host="0.0.0.0",
        port=8000,
        reload=settings.debug,
    )
