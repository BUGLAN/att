# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Audio-to-Text desktop application built around an Electron shell, a Vue renderer, and a FastAPI backend. The active codepath is `electron/` + `src/` + `app/`; the old Python tray and static frontend have been removed.

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
  -> starts the Python backend
  -> owns tray, global input, floating window, clipboard overlay

Vue Renderer (src/)
  -> uses HTTP/WebSocket for backend data
  -> uses preload APIs for desktop integration

FastAPI Backend (app/)
  -> audio processing and ASR orchestration
  -> prompt presets and LLM optimization
  -> records, sessions, clipboard, and settings persistence
```

## Core Modules

- `electron/main.js` - Electron bootstrap, IPC registration, backend lifecycle.
- `electron/input/listener.js` - Global hotkey and clipboard trigger logic.
- `electron/audio/controller.js` - Audio pause/resume integration.
- `electron/python/manager.js` - Python backend process management.
- `src/composables/useRecorder.js` - End-to-end recording workflow.
- `src/composables/useWebSocket.js` - Shared renderer WebSocket client.
- `src/stores/` - Pinia state for app, settings, and records.
- `app/main.py` - FastAPI entrypoint and lifespan setup.
- `app/api/routes.py` - Main API and WebSocket endpoints.
- `app/audio/processor.py` - Audio validation, conversion, persistence.
- `app/stt/` - Whisper / Qwen / 豆包（火山引擎在线流式）providers, runtime services.
  - `app/stt/providers/volcengine_sauc.py` - 豆包 SAUC 二进制协议编解码、definite 前缀推进、错误归一化（纯函数）。
  - `app/stt/providers/doubao_asr_provider.py` - 豆包流式会话与批量转写 provider（在线，无需本地模型）。
- `app/llm/` - LLM config management, gateway, optimizer.
- `app/prompts/` - Prompt preset loading and persistence.
- `app/records/` - Record and session storage.
- `app/clipboard/` - Clipboard APIs and storage.
- `app/config.py` - Environment and persisted user settings.

## Data and Config

- Environment variables are defined in `.env.example`.
- Runtime user settings live in `data/settings.json`.
- Records, sessions, clipboard items, prompts, and LLM configs are JSON-backed under `data/`.

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

1. Recording and transcription are asynchronous.
2. Multi-segment and streaming flows are owned by the Vue renderer plus backend session APIs.
3. Desktop integration flows through Electron IPC, not backend-managed tray clients.
4. Packaging uses `pyproject.toml` as the single Python dependency source.
5. Code comments must be written in Chinese.
6. New comments, documentation, and agent instruction files should default to Chinese first unless there is a clear reason to use another language.
