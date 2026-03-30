"""
FastAPI Application Entry Point
Audio-to-Text API with AI-powered optimization
"""
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse

from app.ai_actions.routes import init_ai_action_service
from app.ai_actions.service import AIActionService
from app.api.routes import close_streaming_session_manager, init_processors
from app.audio.processor import AudioProcessor
from app.clipboard.manager import ClipboardManager
from app.clipboard.routes import init_clipboard_manager
from app.config import settings
from app.llm.gateway import get_gateway
from app.llm.optimizer import TextOptimizer
from app.records.manager import RecordManager
from app.wordbook.manager import WordbookManager
from app.wordbook.routes import init_wordbook_manager
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

    audio_processor = AudioProcessor()
    asr_provider = create_asr_provider(get_asr_provider_config())
    optimizer = TextOptimizer()
    record_manager = RecordManager(settings.data_path)
    clipboard_manager = ClipboardManager(settings.data_path)
    wordbook_manager = WordbookManager(settings.data_path)
    ai_action_service = AIActionService(get_gateway(), wordbook_manager)

    await record_manager.initialize()
    await clipboard_manager.initialize()
    await wordbook_manager.initialize()

    init_processors(audio_processor, asr_provider, optimizer, record_manager)
    init_clipboard_manager(clipboard_manager)
    init_wordbook_manager(wordbook_manager)
    init_ai_action_service(ai_action_service)
    model_service_manager.configure(asr_provider)

    await maybe_preload_asr_provider(asr_provider)

    logger.info("All processors initialized")

    yield

    logger.info("Shutting down Audio-to-Text API")
    await close_streaming_session_manager()
    await model_service_manager.shutdown()
    await qwen_streaming_service_manager.shutdown()
    await asr_provider.close()
    await optimizer.close()


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
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

from app.api.routes import router as api_router
from app.ai_actions.routes import router as ai_action_router
from app.clipboard.routes import router as clipboard_router
from app.wordbook.routes import router as wordbook_router

app.include_router(api_router)
app.include_router(ai_action_router)
app.include_router(clipboard_router)
app.include_router(wordbook_router)

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
