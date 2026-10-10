# syntax=docker/dockerfile:1
# =============================================================================
# 实时 ASR 运行时镜像（后端容器与 ASR 服务容器共用同一镜像）
#
# 设计要点：
# 1. 基础镜像直接使用 vLLM 官方镜像：它与 qwen-asr 锁定的 vllm==0.14.0 精确对应，
#    并且基于 CUDA 13 构建，可覆盖 RTX 5090（sm_120 / Blackwell）。
# 2. 依赖集由 uv 从锁文件导出（uv export），保持 pyproject.toml 为单一依赖来源，
#    不手写依赖清单，避免与本地开发环境漂移。
# 3. 两个服务共用一个镜像：镜像层完全共享，只构建一次；且能保证后端与 ASR
#    服务的运行时版本绝对一致（需要分别升级时再拆分 Dockerfile）。
# 4. 业务数据、模型、日志、密钥一律不进入镜像，运行时由卷挂载提供。
# =============================================================================

ARG VLLM_IMAGE=vllm/vllm-openai:v0.14.0
FROM ${VLLM_IMAGE}

# uv 用于从锁文件安装依赖
COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    UV_SYSTEM_PYTHON=1 \
    UV_LINK_MODE=copy \
    HF_HUB_DISABLE_SYMLINKS_WARNING=1 \
    PYTHONPATH=/app

# 系统依赖：
#   ffmpeg / libsndfile1 / sox —— 音频解码与 pydub、soundfile、sox 包所需
#   curl —— compose 健康检查使用
#   libnuma1 —— vLLM 运行时需要
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        ffmpeg \
        libsndfile1 \
        libnuma1 \
        sox \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 先只拷依赖声明，让依赖层可以被 docker 缓存复用
COPY pyproject.toml uv.lock ./

# 从锁文件导出当前平台的依赖集并安装。
# 基础镜像里已存在 vllm / torch 的相同版本，uv 会识别为已满足而跳过，不会重复下载。
RUN uv export --no-emit-project --frozen --no-dev -o /tmp/requirements.lock.txt \
    && uv pip install --system -r /tmp/requirements.lock.txt \
    && rm -f /tmp/requirements.lock.txt

# 拷贝源码（构建上下文已通过 .dockerignore 排除数据、模型、日志与密钥）
COPY app/ ./app/
COPY scripts/ ./scripts/
COPY prompts/ ./prompts/

# 运行期目录先建好，便于卷挂载后权限正常
RUN mkdir -p /app/data /app/models /app/logs

# 基础镜像的 ENTRYPOINT 是 ["vllm","serve"]，必须清空：
# 否则 compose 里的 command 会被当作它的参数拼成 `vllm serve python3 -m uvicorn ...` 而报错
ENTRYPOINT []

# 默认启动后端；ASR 服务容器在 compose 中覆盖 command
EXPOSE 8000 9000
CMD ["python3", "-m", "uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
