# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Audio-to-Text application with real-time recording, speech recognition, and AI-powered text optimization. Built with FastAPI backend and vanilla JavaScript frontend.

## Commands

```bash
# Install dependencies
pip install -r requirements.txt

# Run development server
uvicorn app.main:app --reload

# Run tests
python tests/test_basic.py

# Download Whisper models (optional, auto-downloads on first use)
python scripts/download_models.py --model large-v3
```

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Web UI     │ ──▶ │  FastAPI    │ ──▶ │  Whisper    │ ──▶ │  DeepSeek   │
│  (static/)  │     │  (app/api)  │     │  (app/stt)  │     │  (app/llm)  │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
```

### Core Modules

- **`app/main.py`** - FastAPI app entry, lifespan management, static file serving
- **`app/api/routes.py`** - REST API endpoints, processors initialized via `init_processors()`
- **`app/audio/processor.py`** - Audio validation, format conversion, chunking. Webm/ogg formats use ffprobe instead of pydub (pydub cannot read webm from memory)
- **`app/stt/transcriber.py`** - Faster-whisper integration with Chinese optimization config. Auto-detects Apple Silicon and uses Core ML acceleration
- **`app/llm/optimizer.py`** - DeepSeek API client with multiple prompt templates
- **`app/records/manager.py`** - JSON-based record storage (one recording = one record)
- **`app/records/models.py`** - Data models: Record, RecordStatus, AudioMetadata, TranscriptionResult

### Frontend

- **`static/index.html`** - Single-page UI
- **`static/js/recorder.js`** - MediaRecorder wrapper. **Important**: Collects chunks during recording, merges and uploads on stop (individual chunks are incomplete webm files)
- **`static/js/app.js`** - App state, API calls, UI updates

## Configuration

Environment variables in `.env` (see `.env.example`):
- `WHISPER_MODEL` - Model size (tiny/base/small/medium/large-v3)
- `WHISPER_DEVICE` - auto/cuda/cpu (auto-detects Apple Silicon → Core ML)
- `WHISPER_COMPUTE_TYPE` - float16/int8 (int8 for CPU/Core ML)
- `DEEPSEEK_API_KEY` - Required for text optimization

## API Endpoints

```
POST /api/audio/upload          # Upload audio (sync transcription)
GET  /api/records               # List records
GET  /api/records/{id}          # Get record details
PUT  /api/records/{id}          # Update record text
DELETE /api/records/{id}        # Delete record
GET  /api/records/{id}/audio    # Serve audio file
POST /api/records/{id}/optimize # AI optimize text
GET  /api/records/{id}/export   # Export (format=txt/json)
GET  /api/health                # Health check
```

## Key Implementation Notes

1. **Transcription is synchronous** - Not using BackgroundTasks to avoid global variable issues in async context

2. **Webm handling** - Browser MediaRecorder produces webm/opus. Pydub cannot read webm from memory pipe, so `validate_audio()` uses ffprobe for webm/ogg files

3. **Recording workflow** - Frontend collects audio chunks during recording, merges into single blob on stop, then uploads. Individual chunks lack container headers and cannot be decoded separately

4. **JSON storage** - Records stored as individual JSON files in `data/records/`. Each record contains: id, audio_path, duration, raw_text, optimized_text, status

5. **Apple Silicon support** - Auto-detects arm64 processor and uses Core ML via CPU with int8 compute type for compatibility
