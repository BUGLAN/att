# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## Project Overview

Audio-to-Text application with real-time recording, speech recognition, and AI-powered text optimization. Built with FastAPI backend, vanilla JavaScript frontend, and a system tray tool for global hotkey recording.

## Commands

```bash
# Install dependencies
pip install -r requirements.txt

# Run development server
uvicorn app.main:app --reload

# Run system tray tool (requires pystray, Pillow, pynput)
python -m tray.main

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
       ▲                   ▲
       │            WebSocket
       │                   │
┌─────────────┐           │
│  Tray Tool  │ ──────────┘
│  (tray/)    │   Hotkey → Recording
└─────────────┘
```

### Core Modules

- **`app/main.py`** - FastAPI app entry, lifespan management, static file serving
- **`app/api/routes.py`** - REST API endpoints, processors initialized via `init_processors()`
- **`app/audio/processor.py`** - Audio validation, format conversion, chunking. Webm/ogg formats use ffprobe instead of pydub (pydub cannot read webm from memory)
- **`app/stt/transcriber.py`** - Faster-whisper integration with Chinese optimization config. Auto-detects Apple Silicon and uses Core ML acceleration
- **`app/llm/optimizer.py`** - DeepSeek API client with multiple prompt templates
- **`app/records/manager.py`** - JSON-based record storage (one recording = one record)
- **`app/records/models.py`** - Data models: Record (includes processing_time), RecordStatus (PENDING/TRANSCRIBING/TRANSCRIBED/OPTIMIZING/OPTIMIZED/FAILED), AudioMetadata, TranscriptionResult
- **`app/services/websocket.py`** - WebSocket connection manager for real-time communication between tray tool and web frontend
- **`app/config.py`** - Settings (env vars) and UserSettings (persistent user config in `data/settings.json`)

### Tray Tool (`tray/`)

System tray application for global hotkey recording control:
- **`tray/main.py`** - Tray app entry, icon management, event loop coordination
- **`tray/hotkey.py`** - Global hotkey listener (configurable via settings, default: Alt+Cmd on macOS, Alt+Ctrl on Windows). Uses pynput on macOS (more reliable). Supports dynamic hotkey updates via WebSocket
- **`tray/websocket_client.py`** - WebSocket client connecting to backend for recording commands and hotkey updates
- **`tray/audio_controller.py`** - Pauses/resumes media playback during recording via AppleScript media keys (macOS only)

### Frontend

- **`static/index.html`** - Single-page UI with sidebar navigation
- **`static/js/recorder.js`** - MediaRecorder wrapper. **Important**: Collects chunks during recording, merges and uploads on stop (individual chunks are incomplete webm files)
- **`static/js/app.js`** - App state, API calls, UI updates, page switching
- **`static/js/websocket.js`** - WebSocket client with auto-reconnect and heartbeat
- **`static/js/settings.js`** - Settings management, loads/saves via `/api/settings`

## Configuration

Environment variables in `.env` (see `.env.example`):
- `WHISPER_MODEL` - Model size (tiny/base/small/medium/large-v3)
- `WHISPER_DEVICE` - auto/cuda/cpu (auto-detects Apple Silicon → Core ML)
- `WHISPER_COMPUTE_TYPE` - float16/int8 (int8 for CPU/Core ML)
- `DEEPSEEK_API_KEY` - Required for text optimization

User settings in `data/settings.json` (persisted via `/api/settings`):
- `whisper_model`, `whisper_device` - Model selection
- `pause_audio_on_recording` - Auto-pause media during recording
- `auto_optimize` - Auto-optimize text after transcription
- `hotkey` - Configurable recording hotkey (platform-specific default: Alt+Cmd on macOS, Alt+Ctrl on Windows)

## API Endpoints

```
POST /api/audio/upload          # Upload audio (async transcription)
GET  /api/records               # List records
GET  /api/records/{id}          # Get record details
GET  /api/records/{id}/status   # Get transcription status (for polling)
PUT  /api/records/{id}          # Update record text
DELETE /api/records/{id}        # Delete record
GET  /api/records/{id}/audio    # Serve audio file
POST /api/records/{id}/optimize # AI optimize text
GET  /api/records/{id}/export   # Export (format=txt/json)
GET  /api/settings              # Get user settings
PUT  /api/settings              # Update user settings
GET  /api/health                # Health check
WS   /api/ws/{client_type}      # WebSocket (client_type: "web" or "tray")
```

## Key Implementation Notes

1. **Transcription is async** - Upload returns immediately with `record_id`, frontend polls `/api/records/{id}/status` for completion

2. **Webm handling** - Browser MediaRecorder produces webm/opus. Pydub cannot read webm from memory pipe, so `validate_audio()` uses ffprobe for webm/ogg files

3. **Recording workflow** - Frontend collects audio chunks during recording, merges into single blob on stop, then uploads. Individual chunks lack container headers and cannot be decoded separately

4. **JSON storage** - Records stored as individual JSON files in `data/records/`. Each record contains: id, audio_path, duration, raw_text, optimized_text, status, processing_time

5. **Status flow** - Record status transitions:
   - Normal: `pending` → `transcribing` → `transcribed`
   - With auto-optimization: `pending` → `transcribing` → `transcribed` → `optimizing` → `optimized`
   - Frontend polling must check for both `transcribed` and `optimized` as completion states

6. **Apple Silicon support** - Auto-detects arm64 processor and uses Core ML via CPU with int8 compute type for compatibility

7. **WebSocket message flow**:
   - Tray tool sends `{"type": "recording", "action": "start|stop"}` → Server broadcasts to web clients
   - Web clients send `{"type": "status_update", ...}` → Server tracks and broadcasts status
   - Web clients send `{"type": "hotkey_update", "hotkey": {...}}` → Server validates, saves, and broadcasts to tray clients
   - Server sends `{"type": "hotkey_changed", "hotkey": {...}}` → Tray tool updates its hotkey listener

8. **Tray tool threading** - pystray runs blocking in main thread, asyncio event loop runs in separate daemon thread. Hotkey callbacks must use `asyncio.run_coroutine_threadsafe()` to schedule async work

## Code Style

- **Comments in Chinese** - 代码注释必须使用中文编写
