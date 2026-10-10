#!/bin/sh
# =============================================================================
# ASR 服务容器入口
#
# 职责：启动流式桥接服务，并在服务可用后自动预热模型。
#
# 为什么需要自动预热：
#   容器可能被 Docker Desktop 自动拉起（开机自启、崩溃后 restart 策略、手动 up），
#   这些场景下没有人会去执行一键启动脚本的第 8 步。若不自动预热，模型将一直处于
#   「未加载」状态，而前端会把该状态渲染成「模型加载中」并禁用开启按钮，
#   用户会陷入「用不了又点不动」的死局（实测踩过）。
#
# 关闭方式：QWEN_STREAMING_AUTO_PRELOAD=0
# =============================================================================
set -u

HOST="${QWEN_STREAMING_HOST:-0.0.0.0}"
PORT="${QWEN_STREAMING_PORT:-9000}"
AUTO_PRELOAD="${QWEN_STREAMING_AUTO_PRELOAD:-1}"

python3 -m uvicorn scripts.qwen_streaming_service:app --host "$HOST" --port "$PORT" &
UVICORN_PID=$!

# 收到停止信号时把 uvicorn 一起带走，避免残留进程
trap 'kill -TERM "$UVICORN_PID" 2>/dev/null || true' TERM INT

if [ "$AUTO_PRELOAD" != "0" ]; then
    # 等服务端口可用，最多等 120 秒
    attempt=0
    while [ "$attempt" -lt 60 ]; do
        if curl -fsS --max-time 3 "http://127.0.0.1:${PORT}/api/health" >/dev/null 2>&1; then
            break
        fi
        attempt=$((attempt + 1))
        sleep 2
    done

    echo "[entrypoint] 开始自动预热模型（加载权重 + 引擎编译，首次约 1-2 分钟）"
    if curl -fsS -X POST --max-time 900 "http://127.0.0.1:${PORT}/api/preload" >/dev/null 2>&1; then
        echo "[entrypoint] 模型预热完成，流式服务已就绪"
    else
        echo "[entrypoint] 模型预热失败；服务仍在运行，可通过前端「开启模型」重试" >&2
    fi
fi

wait "$UVICORN_PID"
