"""
FastAPI Application Entry Point
Audio-to-Text API with AI-powered optimization
"""
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
from app.records.manager import RecordManager
from app.api.routes import init_processors
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

    # Initialize directories
    await record_manager.initialize()

    # Pass to routes
    init_processors(audio_processor, transcriber, optimizer, record_manager)

    logger.info("All processors initialized")

    yield

    # Shutdown
    logger.info("Shutting down Audio-to-Text API")
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
app.include_router(api_router)

# Mount static files
static_dir = Path(__file__).parent.parent / "static"
if static_dir.exists():
    app.mount("/static", StaticFiles(directory=str(static_dir)), name="static")


@app.get("/")
async def root():
    """Root endpoint - Serve index page"""
    index_path = static_dir / "index.html"
    if index_path.exists():
        return FileResponse(str(index_path))
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
