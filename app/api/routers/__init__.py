"""按资源拆分的 APIRouter 集合（每个子路由自带 ``prefix="/api"`` 与 tags）。

- `records.py`：音频上传 + 记录 CRUD/优化/导出
- `sessions.py`：多段录音会话 + WebSocket（/api/ws、/api/asr/stream）
- `settings.py`：用户设置 + 健康检查 + 模型服务启停
- `prompts.py`：提示词预设
- `llm.py`：LLM 配置与 provider 元数据

聚合层见 `app/api/routes.py`（保留既有公开符号）与 `app/main.py`（逐个 include_router）。
"""
