# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## Project Overview

Audio-to-Text desktop application built around an Electron shell, a Vue renderer, and a FastAPI backend. The current production path is `electron/` + `src/` + `app/`; legacy Python tray and static frontend code have been removed.

## Commands

```bash
# Install Python dependencies
uv sync

# Install frontend dependencies
npm install

# Run backend only
npm run backend:dev:uv

# Run Electron + Vite renderer
npm run electron:dev

# Build renderer bundle
npm run build

# Package Electron app
node scripts/package-electron.js --win

# Run tests
uv run python -m pytest tests -q
```

## Architecture

```text
Electron Main (electron/)
  -> starts Python backend through electron/python/manager.js
  -> starts local Python desktop agent for global hotkey and recording mute
  -> manages tray, floating window, clipboard overlay

Vue Renderer (src/)
  -> talks to FastAPI over HTTP/WebSocket
  -> uses Electron preload APIs for desktop-only capabilities

FastAPI Backend (app/)
  -> audio ingestion and conversion
  -> ASR provider orchestration
  -> text optimization and prompt presets
  -> JSON-backed records, sessions, clipboard storage
```

## Core Modules

- `electron/main.js` - Electron app entry, backend startup, IPC wiring.
- `electron/desktop/settings.js` - Local desktop settings storage for hotkey and mute policy.
- `electron/desktopAgent/manager.js` - Starts and monitors the local Python desktop agent.
- `electron/audio/controller.js` - Pause/resume desktop audio during recording as a fallback path.
- `electron/python/desktop_agent.py` - Local desktop agent for hotkey press/release and mute control.
- `electron/python/manager.js` - Starts and monitors the bundled Python backend.
- `src/composables/useRecorder.js` - Main recording workflow for single, multi, and streaming modes.
- `src/composables/useWebSocket.js` - Shared WebSocket client for renderer status sync.
- `src/components/settings/SettingsPage.vue` - Settings UI, desktop sync, provider config.
- `app/main.py` - FastAPI app entry and lifespan wiring.
- `app/api/routes.py` - REST and WebSocket API endpoints.
- `app/audio/processor.py` - Audio validation, conversion, and persistence.
- `app/stt/` - Whisper and Qwen ASR provider integrations.
- `app/llm/` - LLM gateway, config management, and optimizer logic.
- `app/records/` - JSON-backed record/session storage.
- `app/clipboard/` - Clipboard item storage and API routes.
- `app/config.py` - Environment settings and persisted user settings.

## Configuration

Environment variables live in `.env` and examples are in `.env.example`.

Important backend settings:
- `ASR_PROVIDER`
- `ASR_PRELOAD_ON_STARTUP`
- `WHISPER_MODEL`, `WHISPER_DEVICE`, `WHISPER_COMPUTE_TYPE`
- `QWEN_ASR_*`
- `QWEN_STREAMING_*`

Business settings are persisted in `data/settings.json` and include:
- Recording mode and silence thresholds
- Clipboard overlay behavior
- Auto-optimize / auto-copy toggles
- Default prompt preset

Desktop-local settings are persisted by Electron separately and include:
- Hotkey configuration
- Pause-audio-on-recording switch

## API Surface

```text
POST /api/audio/upload
GET  /api/records
GET  /api/records/search
GET  /api/records/{id}
GET  /api/records/{id}/status
PUT  /api/records/{id}
DELETE /api/records/{id}
GET  /api/records/{id}/audio
GET  /api/records/{id}/export
POST /api/records/{id}/optimize

POST /api/sessions
GET  /api/sessions/{id}
POST /api/sessions/{id}/segments
POST /api/sessions/{id}/merge
DELETE /api/sessions/{id}

GET  /api/settings
PUT  /api/settings
GET  /api/health
WS   /api/ws
WS   /api/asr/stream
```

## Key Notes

1. Recording uploads are asynchronous; completion is tracked through polling and WebSocket events.
2. Browser recordings are uploaded as merged blobs; individual MediaRecorder chunks are not independently usable.
3. Records, sessions, clipboard items, prompts, and LLM configs are stored under `data/` as JSON files.
4. Desktop-only capabilities are implemented through Electron IPC; global hotkey and recording mute are provided by the local Python desktop agent, not the business backend.
5. Code comments must be written in Chinese.
6. New comments, documentation, and agent instruction files should default to Chinese first unless there is a clear reason to use another language.
