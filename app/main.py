"""
FastAPI Application Entry Point
Audio-to-Text API with AI-powered optimization
"""
import asyncio
from contextlib import asynccontextmanager
from pathlib import Path
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from fastapi.responses import FileResponse

from app.config import settings
from app.audio.processor import AudioProcessor
from app.stt.transcriber import WhisperTranscriber
from app.llm.optimizer import TextOptimizer
from app.clipboard.manager import ClipboardManager
from app.records.manager import RecordManager
from app.api.routes import init_processors
from app.clipboard.routes import init_clipboard_manager
from app.services.hotkey_listener import init_hotkey_service
from app.utils.logging import setup_logging, get_logger

logger = get_logger(__name__)

# Setup logging
setup_logging()


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan - startup and shutdown events"""
    # Startup
    logger.info(
        "Starting Audio-to-Text API",
        app_name=settings.app_name,
        debug=settings.debug
    )

    # Initialize processors
    audio_processor = AudioProcessor()
    transcriber = WhisperTranscriber()
    optimizer = TextOptimizer()
    record_manager = RecordManager(settings.data_path)
    clipboard_manager = ClipboardManager(settings.data_path)

    # Initialize directories
    await record_manager.initialize()
    await clipboard_manager.initialize()

    # Pass to routes
    init_processors(audio_processor, transcriber, optimizer, record_manager)
    init_clipboard_manager(clipboard_manager)

    # Start backend hotkey listener after runtime state is ready.
    hotkey_service = init_hotkey_service(asyncio.get_running_loop())
    hotkey_service.start()

    # Preload model to eliminate first-request latency
    logger.info("Preloading Whisper model...")
    transcriber._load_model()

    logger.info("All processors initialized")

    yield

    # Shutdown
    logger.info("Shutting down Audio-to-Text API")
    hotkey_service.stop()
    transcriber.unload_model()
    await optimizer.close()


# Create FastAPI application
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
    lifespan=lifespan
)

# CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Include routers
from app.api.routes import router as api_router
from app.clipboard.routes import router as clipboard_router
app.include_router(api_router)
app.include_router(clipboard_router)

# Frontend directories
project_root = Path(__file__).parent.parent
static_dir = project_root / "static"
dist_dir = project_root / "dist"
dist_assets_dir = dist_dir / "assets"

# Mount legacy static files for reference during migration
if static_dir.exists():
    app.mount("/static", StaticFiles(directory=str(static_dir)), name="static")

# Mount built Vue assets when available
if dist_assets_dir.exists():
    app.mount("/assets", StaticFiles(directory=str(dist_assets_dir)), name="assets")


@app.get("/")
async def root():
    """Root endpoint - Serve the Vue frontend when available."""
    dist_index = dist_dir / "index.html"
    if dist_index.exists():
        return FileResponse(str(dist_index))

    static_index = static_dir / "index.html"
    if static_index.exists():
        return FileResponse(str(static_index))

    return {
        "name": settings.app_name,
        "version": "0.1.0",
        "docs": "/docs",
        "health": "/api/health"
    }


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(
        "app.main:app",
        host="0.0.0.0",
        port=8000,
        reload=settings.debug
    )
