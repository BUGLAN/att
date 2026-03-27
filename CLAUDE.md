# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Audio-to-Text application with real-time recording, speech recognition, and AI-powered text optimization. Built with FastAPI backend, vanilla JavaScript frontend, and a system tray tool for global hotkey recording. Features multi-segment recording with silence detection, clipboard management, and multi-provider LLM support.

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
│  Web UI     │ ──▶ │  FastAPI    │ ──▶ │  Whisper    │ ──▶ │  LLM Gateway│
│  (static/)  │     │  (app/api)  │     │  (app/stt)  │     │  (app/llm)  │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
       ▲                   ▲                                       │
       │            WebSocket                              ┌──────┴──────┐
       │                   │                               │             │
┌─────────────┐           │                          DeepSeek   Claude   OpenAI
│  Tray Tool  │ ──────────┘                               │    ...      │
│  (tray/)    │   Hotkey → Recording                      └─────────────┘
└─────────────┘
```

### Core Modules

- **`app/main.py`** - FastAPI app entry, lifespan management, static file serving
- **`app/api/routes.py`** - REST API endpoints, processors initialized via `init_processors()`
- **`app/audio/processor.py`** - Audio validation, format conversion, chunking. Webm/ogg formats use ffprobe instead of pydub (pydub cannot read webm from memory)
- **`app/stt/transcriber.py`** - Faster-whisper integration with Chinese optimization config. Auto-detects Apple Silicon and uses Core ML acceleration
- **`app/llm/optimizer.py`** - LLM text optimizer using LLMGateway
- **`app/llm/gateway.py`** - Factory for creating LLM provider instances, caches providers by config_id
- **`app/llm/config_manager.py`** - Manages multiple LLM configurations with encrypted API keys in `data/llm_configs.json`
- **`app/llm/models.py`** - LLMProvider enum (DEEPSEEK, CLAUDE, OPENAI, DOUBAO, QWEN, CUSTOM), LLMConfig model
- **`app/llm/providers/`** - Provider implementations: deepseek.py, claude.py, openai.py, doubao.py, qwen.py
- **`app/prompts/manager.py`** - Manages prompt presets from `prompts/*.txt` files and custom presets in `data/prompts/presets.json`
- **`app/records/manager.py`** - JSON-based record storage (one recording = one record)
- **`app/records/session_manager.py`** - Multi-segment recording sessions in `data/sessions/`
- **`app/records/models.py`** - Data models: Record, RecordStatus, RecordingSession, AudioSegment
- **`app/clipboard/manager.py`** - Clipboard item storage in `data/clipboard/`
- **`app/clipboard/routes.py`** - Clipboard API endpoints
- **`app/services/websocket.py`** - WebSocket connection manager for real-time communication
- **`app/utils/encryption.py`** - AES encryption for API keys
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
- **`static/js/multi_segment_recorder.js`** - Multi-segment recorder with silence detection. Uses SilenceDetector to auto-split segments during continuous recording
- **`static/js/silence_detector.js`** - Audio analysis for silence detection using Web Audio API
- **`static/js/app.js`** - App state, API calls, UI updates, page switching
- **`static/js/websocket.js`** - WebSocket client with auto-reconnect and heartbeat
- **`static/js/settings.js`** - Settings management, loads/saves via `/api/settings`

## Configuration

Environment variables in `.env` (see `.env.example`):
- `WHISPER_MODEL` - Model size (tiny/base/small/medium/large-v3)
- `WHISPER_DEVICE` - auto/cuda/cpu (auto-detects Apple Silicon → Core ML)
- `WHISPER_COMPUTE_TYPE` - float16/int8 (int8 for CPU/Core ML)
- `DATA_PATH` - Data directory path (default: `data`)

User settings in `data/settings.json` (persisted via `/api/settings`):
- `whisper_model`, `whisper_device` - Model selection
- `pause_audio_on_recording` - Auto-pause media during recording
- `auto_optimize` - Auto-optimize text after transcription
- `hotkey` - Configurable recording hotkey (platform-specific default: Alt+Cmd on macOS, Alt+Ctrl on Windows)
- `default_preset_id`, `last_used_preset_id` - Prompt preset preferences

LLM configurations in `data/llm_configs.json`:
- Multiple provider configs with encrypted API keys
- Supports: DeepSeek, Claude, OpenAI, Doubao, Qwen, Custom (OpenAI-compatible)
- First config is automatically set as default

## API Endpoints

```
# Audio & Records
POST /api/audio/upload          # Upload audio (async transcription)
GET  /api/records               # List records
GET  /api/records/{id}          # Get record details
GET  /api/records/{id}/status   # Get transcription status (for polling)
PUT  /api/records/{id}          # Update record text
DELETE /api/records/{id}        # Delete record
GET  /api/records/{id}/audio    # Serve audio file
POST /api/records/{id}/optimize # AI optimize text
GET  /api/records/{id}/export   # Export (format=txt/json)

# Multi-segment Sessions
POST /api/sessions              # Create recording session
POST /api/sessions/{id}/segments # Upload segment audio
POST /api/sessions/{id}/merge   # Merge all segments into record

# Clipboard
GET  /api/clipboard             # List clipboard items
POST /api/clipboard             # Create clipboard item
GET  /api/clipboard/{id}        # Get clipboard item
PUT  /api/clipboard/{id}        # Update clipboard item
DELETE /api/clipboard/{id}      # Delete clipboard item
GET  /api/clipboard/stats       # Get clipboard statistics

# LLM Configs
GET  /api/llm-configs           # List all LLM configs
POST /api/llm-configs           # Create LLM config
PUT  /api/llm-configs/{id}      # Update LLM config
DELETE /api/llm-configs/{id}    # Delete LLM config
PUT  /api/llm-configs/{id}/default # Set as default

# Prompts
GET  /api/prompts               # List all prompt presets
POST /api/prompts               # Create custom preset
PUT  /api/prompts/{id}          # Update custom preset
DELETE /api/prompts/{id}        # Delete custom preset
POST /api/prompts/{id}/default  # Set as default
POST /api/prompts/preview       # Preview prompt with text

# Settings & Health
GET  /api/settings              # Get user settings
PUT  /api/settings              # Update user settings
GET  /api/health                # Health check
WS   /api/ws/{client_type}      # WebSocket (client_type: "web" or "tray")
```

## Key Implementation Notes

1. **Transcription is async** - Upload returns immediately with `record_id`, frontend polls `/api/records/{id}/status` for completion

2. **Webm handling** - Browser MediaRecorder produces webm/opus. Pydub cannot read webm from memory pipe, so `validate_audio()` uses ffprobe for webm/ogg files

3. **Recording workflow** - Frontend collects audio chunks during recording, merges into single blob on stop, then uploads. Individual chunks lack container headers and cannot be decoded separately

4. **Multi-segment recording** - `MultiSegmentRecorder` in `static/js/multi_segment_recorder.js` uses `SilenceDetector` to auto-split during continuous recording. Each segment uploads to `/api/sessions/{id}/segments` and transcribes in real-time

5. **JSON storage** - Records stored as individual JSON files in `data/records/`. Each record contains: id, audio_path, duration, raw_text, optimized_text, status, processing_time

6. **Status flow** - Record status transitions:
   - Normal: `pending` → `transcribing` → `transcribed`
   - With auto-optimization: `pending` → `transcribing` → `transcribed` → `optimizing` → `optimized`
   - Frontend polling must check for both `transcribed` and `optimized` as completion states

7. **Apple Silicon support** - Auto-detects arm64 processor and uses Core ML via CPU with int8 compute type for compatibility

8. **WebSocket message flow**:
   - Tray tool sends `{"type": "recording", "action": "start|stop"}` → Server broadcasts to web clients
   - Web clients send `{"type": "status_update", ...}` → Server tracks and broadcasts status
   - Web clients send `{"type": "hotkey_update", "hotkey": {...}}` → Server validates, saves, and broadcasts to tray clients
   - Server sends `{"type": "hotkey_changed", "hotkey": {...}}` → Tray tool updates its hotkey listener

9. **Tray tool threading** - pystray runs blocking in main thread, asyncio event loop runs in separate daemon thread. Hotkey callbacks must use `asyncio.run_coroutine_threadsafe()` to schedule async work

10. **Multi-provider LLM** - `LLMGateway` creates provider instances from `LLMConfig`. API keys are encrypted with AES in `data/llm_configs.json`. Custom provider uses OpenAI-compatible API

11. **Prompt presets** - Built-in presets loaded from `prompts/*.txt` (read-only), custom presets stored in `data/prompts/presets.json`. Templates use `{text}` placeholder

## Code Style

- **Comments in Chinese** - 代码注释必须使用中文编写
