"""本地文件转写 HTTP 接口（外部采集层脚本 + App 内「转文字」页共用）。

路由只做解析、鉴权与状态码映射，业务全部在 FileTranscriptionService：

- POST /api/transcribe          短文件同步 200；长文件 202 {"job_id"}
- GET  /api/transcribe/jobs/{id} 轮询异步任务：{"status","result","error"}

错误码契约：404 文件不存在 / 415 无法解码 / 413 超时长上限 /
503 引擎忙（Retry-After）/ 401 token 不匹配 / 500 其他内部错误。
"""

from __future__ import annotations

import secrets

from fastapi import APIRouter, Depends, Header, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

from app.config import settings
from app.services.file_transcription_service import (
    FileTranscribeError,
    file_transcription_service,
)

router = APIRouter(prefix="/api", tags=["api", "transcribe-file"])


async def _require_token(
    x_att_token: str | None = Header(default=None),
) -> None:
    """可选鉴权：transcribe_file_api_token 非空时要求 X-ATT-Token 匹配。

    用 compare_digest 避免时序侧信道；默认（未配置 token）回环监听即不鉴权。
    """
    expected = settings.transcribe_file_api_token
    if expected and not secrets.compare_digest(x_att_token or "", expected):
        raise HTTPException(status_code=401, detail="缺少或错误的 X-ATT-Token")


class TranscribeFileRequest(BaseModel):
    """POST /api/transcribe 请求体。"""

    path: str = Field(..., description="本地媒体文件绝对路径（音频或视频容器均可）")
    language: str = Field("zh", description="识别语言，默认 zh")
    want_segments: bool = Field(False, description="true 时响应附带分段时间戳")


@router.post("/transcribe", dependencies=[Depends(_require_token)])
async def transcribe_local_file(request: TranscribeFileRequest):
    """转写本地文件：缓存命中秒回；短文件同步返回；长文件返回 202 任务号。"""
    try:
        outcome = await file_transcription_service.handle_transcribe(
            request.path,
            request.language,
            request.want_segments,
        )
    except FileTranscribeError as error:
        raise HTTPException(
            status_code=error.status_code,
            detail=error.message,
            headers=error.headers,
        ) from error

    if outcome.kind == "job":
        return JSONResponse(status_code=202, content=outcome.payload)
    return outcome.payload


@router.get("/transcribe/jobs/{job_id}", dependencies=[Depends(_require_token)])
async def get_transcribe_job(job_id: str):
    """轮询异步转写任务；未知或已过期的任务号返回 404。"""
    job = file_transcription_service.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="任务不存在或已过期")
    return {
        "status": job["status"],
        "result": job["result"],
        "error": job["error"],
    }
