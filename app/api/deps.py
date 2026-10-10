"""路由层运行时依赖入口（服务定位器）。

拆分 `app/api/routes.py` 后，全局实例（AudioProcessor / ASRProvider / TextOptimizer /
RecordManager / UserSettings / 配置 / 流式会话管理器）**仍然由聚合层模块
``app.api.routes`` 持有**，原因：

1. `app.main` 通过 `routes.init_processors(...)` 注入实例，并向
   `app.api.routes.asr_provider` 之类的模块属性写入重建后的 provider；
2. 既有测试（tests/test_api_records.py、test_api_settings.py、test_api_stream_ws.py、
   test_path_safety.py）通过 ``monkeypatch.setattr(app.api.routes, "record_manager", ...)``
   替换实例。

因此子路由**不能在 import 阶段把实例固化进自己的命名空间**，否则上面的注入与替换
全部失效。这里统一改为「每次请求都从聚合层读一次」：

.. code-block:: python

    @router.get("/records")
    async def list_records(state=Depends(runtime_state)):
        record_manager = state.record_manager

``runtime_state()`` 内部延迟 import，避免子路由与聚合层形成循环 import。
"""

from __future__ import annotations

from types import ModuleType


def runtime_state() -> ModuleType:
    """返回聚合层模块 `app.api.routes`（运行时状态的唯一宿主）。"""
    from app.api import routes as api_routes

    return api_routes
