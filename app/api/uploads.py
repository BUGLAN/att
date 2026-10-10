"""上传体积限流助手：在把请求体读进内存之前校验大小。

`POST /api/audio/upload` 与 `POST /api/sessions/{id}/segments` 共用这里的能力
（原实现内联在 `app/api/routes.py`，拆分后集中到本模块）：

先看 ``Content-Length`` 快速拒绝；再按块读取并累计字节数，一旦超过上限立即中断，
因此内存占用上界是 limit + 一个块，而不是整个请求体。
"""

from __future__ import annotations

from fastapi import HTTPException, Request, UploadFile

# 上传分块大小：1 MiB，用于把「全量 read() 入内存」改成边读边校验上限
UPLOAD_CHUNK_SIZE = 1024 * 1024


def describe_size_limit(limit: int) -> str:
    """把字节上限格式化成用户可读的 MB 文案。"""
    return f"{limit / (1024 * 1024):.1f} MB"


async def read_upload_within_limit(request: Request, file: UploadFile, limit: int) -> bytes:
    """在把请求体读进内存之前校验大小，超限直接 413，避免大文件 OOM。"""
    declared_length = request.headers.get("content-length")
    if declared_length and declared_length.isdigit() and int(declared_length) > limit:
        raise HTTPException(
            status_code=413,
            detail=f"上传文件过大：请求体超过上限 {describe_size_limit(limit)}"
        )

    chunks = []
    total_size = 0
    while True:
        chunk = await file.read(UPLOAD_CHUNK_SIZE)
        if not chunk:
            break
        total_size += len(chunk)
        if total_size > limit:
            raise HTTPException(
                status_code=413,
                detail=f"上传文件过大：音频超过上限 {describe_size_limit(limit)}"
            )
        chunks.append(chunk)

    return b"".join(chunks)
