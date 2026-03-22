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
python scripts/download_models.py --model tiny
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
- **`app/stt/transcriber.py`** - Faster-whisper integration with Chinese optimization config
- **`app/llm/optimizer.py`** - DeepSeek API client with multiple prompt templates (formal, meeting, novel, command)
- **`app/fragments/manager.py`** - SQLite-based fragment CRUD, merging, export (TXT/JSON/SRT)

### Frontend

- **`static/index.html`** - Single-page UI
- **`static/js/recorder.js`** - MediaRecorder wrapper. **Important**: Collects chunks during recording, merges and uploads on stop (individual chunks are incomplete webm files)
- **`static/js/app.js`** - App state, API calls, UI updates

## Configuration

Environment variables in `.env`:
- `WHISPER_MODEL` - Model size (tiny/base/small/medium/large-v3)
- `WHISPER_DEVICE` - auto/cuda/cpu
- `DEEPSEEK_API_KEY` - Required for text optimization

## API Endpoints

```
POST /api/sessions              # Create session
POST /api/audio/upload          # Upload audio (sync transcription)
GET  /api/fragments?session_id= # List fragments
POST /api/fragments/{id}/optimize  # AI optimize text
POST /api/fragments/merge       # Merge fragments
GET  /api/export/{session_id}   # Export (format=txt/json/srt)
```

## Key Implementation Notes

1. **Transcription is synchronous** - Not using BackgroundTasks to avoid global variable issues in async context

2. **Webm handling** - Browser MediaRecorder produces webm/opus. Pydub cannot read webm from memory pipe, so `validate_audio()` uses ffprobe for webm/ogg files

3. **Recording workflow** - Frontend collects audio chunks during recording, merges into single blob on stop, then uploads. Individual chunks lack container headers and cannot be decoded separately

4. **Database** - SQLite with aiosqlite. Schema auto-created on first run via `FragmentManager.initialize()`
