"""
Text optimization module using DeepSeek API
Converts spoken language to written text
"""
import httpx
from typing import Optional, List
from dataclasses import dataclass
import asyncio
from tenacity import (
    retry,
    stop_after_attempt,
    wait_exponential,
    retry_if_exception_type
)
import time

from app.config import settings
from app.utils.logging import get_logger

logger = get_logger(__name__)


@dataclass
class OptimizationResult:
    """Result of text optimization"""
    original_text: str
    optimized_text: str
    processing_time: float
    tokens_used: int = 0


class TextOptimizer:
    """
    DeepSeek API client for text optimization

    Features:
    - Converts spoken language to written text
    - Removes filler words and improves grammar
    - Batch processing for multiple fragments
    - Retry logic with exponential backoff
    """

    # Prompt templates for different optimization styles
    PROMPTS = {
        "formal": """你是一位专业文字编辑。请将以下口语化文字转换为书面语。

要求：
1. 保持原意，不改变事实内容
2. 去除口语填充词（如"那个"、"嗯"、"然后"、"就是说"等）
3. 修正语法错误和不通顺的表达
4. 调整标点符号，使文章通顺
5. 保持适当的段落结构
6. 不要添加原文没有的内容

原文：
{text}

优化后的文字：""",

        "meeting": """你是一位会议记录专家。请将以下口语化的会议内容整理成规范的会议记录。

要求：
1. 提取关键信息和要点
2. 去除无关的口语表达
3. 保持发言者的原意
4. 使用简洁明了的书面语
5. 适当分段，提高可读性

原文：
{text}

整理后的内容：""",

        "novel": """你是一位文学编辑。请将以下口述内容整理成适合小说阅读的文字。

要求：
1. 保持故事情节和细节
2. 润色语言，使其更加流畅
3. 去除口语化的表达
4. 保持叙述的连贯性
5. 注意场景和对话的区分

原文：
{text}

整理后的文字：""",

        "command": """请将以下语音指令转换为简洁明确的文字命令。

要求：
1. 提取核心指令内容
2. 去除无关的口语表达
3. 使用祈使句或简洁陈述句
4. 保持指令的完整性

原文：
{text}

转换后的命令："""
    }

    def __init__(
        self,
        api_key: Optional[str] = None,
        model: Optional[str] = None,
        base_url: Optional[str] = None
    ):
        """
        Initialize the optimizer

        Args:
            api_key: DeepSeek API key (default from settings)
            model: Model name (default from settings)
            base_url: API base URL (default from settings)
        """
        self.api_key = api_key or settings.deepseek_api_key
        self.model = model or settings.deepseek_model
        self.base_url = base_url or settings.deepseek_base_url

        if not self.api_key:
            logger.warning("DeepSeek API key not configured")

        self._client = httpx.AsyncClient(
            base_url=self.base_url,
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json"
            },
            timeout=60.0
        )

        logger.info(
            "TextOptimizer initialized",
            model=self.model,
            base_url=self.base_url
        )

    @retry(
        stop=stop_after_attempt(3),
        wait=wait_exponential(multiplier=1, min=1, max=10),
        retry=retry_if_exception_type((httpx.HTTPError, asyncio.TimeoutError)),
        reraise=True
    )
    async def optimize(
        self,
        text: str,
        style: str = "formal"
    ) -> OptimizationResult:
        """
        Optimize text using DeepSeek API

        Args:
            text: Original text to optimize
            style: Optimization style (formal, meeting, novel, command)

        Returns:
            OptimizationResult with optimized text
        """
        if not self.api_key:
            raise ValueError("DeepSeek API key not configured")

        if not text.strip():
            return OptimizationResult(
                original_text=text,
                optimized_text=text,
                processing_time=0
            )

        prompt_template = self.PROMPTS.get(style, self.PROMPTS["formal"])
        prompt = prompt_template.format(text=text)

        logger.debug(
            "Optimizing text",
            style=style,
            text_length=len(text)
        )

        start_time = time.time()

        response = await self._client.post(
            "/chat/completions",
            json={
                "model": self.model,
                "messages": [
                    {"role": "user", "content": prompt}
                ],
                "temperature": settings.deepseek_temperature,
                "max_tokens": settings.deepseek_max_tokens
            }
        )

        response.raise_for_status()
        data = response.json()

        processing_time = time.time() - start_time

        optimized_text = data["choices"][0]["message"]["content"]
        tokens_used = data.get("usage", {}).get("total_tokens", 0)

        logger.info(
            "Text optimized",
            style=style,
            original_length=len(text),
            optimized_length=len(optimized_text),
            processing_time=f"{processing_time:.2f}s",
            tokens_used=tokens_used
        )

        return OptimizationResult(
            original_text=text,
            optimized_text=optimized_text,
            processing_time=processing_time,
            tokens_used=tokens_used
        )

    async def batch_optimize(
        self,
        texts: List[str],
        style: str = "formal",
        batch_size: int = 5
    ) -> List[OptimizationResult]:
        """
        Optimize multiple texts in batches

        Args:
            texts: List of texts to optimize
            style: Optimization style
            batch_size: Number of concurrent requests

        Returns:
            List of OptimizationResults
        """
        results = []
        semaphore = asyncio.Semaphore(batch_size)

        async def optimize_with_semaphore(text: str) -> OptimizationResult:
            async with semaphore:
                try:
                    return await self.optimize(text, style)
                except Exception as e:
                    logger.error("Batch optimization failed", error=str(e))
                    return OptimizationResult(
                        original_text=text,
                        optimized_text=text,
                        processing_time=0
                    )

        tasks = [optimize_with_semaphore(text) for text in texts]
        results = await asyncio.gather(*tasks)

        return list(results)

    async def optimize_long_text(
        self,
        text: str,
        style: str = "formal",
        chunk_size: int = 2000
    ) -> OptimizationResult:
        """
        Optimize long text by splitting into chunks

        Args:
            text: Long text to optimize
            style: Optimization style
            chunk_size: Maximum characters per chunk

        Returns:
            OptimizationResult with merged optimized text
        """
        if len(text) <= chunk_size:
            return await self.optimize(text, style)

        # Split text into chunks at sentence boundaries
        chunks = self._split_text(text, chunk_size)

        # Optimize each chunk
        results = await self.batch_optimize(chunks, style)

        # Merge results
        merged_text = "\n\n".join(r.optimized_text for r in results)
        total_time = sum(r.processing_time for r in results)
        total_tokens = sum(r.tokens_used for r in results)

        return OptimizationResult(
            original_text=text,
            optimized_text=merged_text,
            processing_time=total_time,
            tokens_used=total_tokens
        )

    def _split_text(
        self,
        text: str,
        chunk_size: int
    ) -> List[str]:
        """
        Split text into chunks at sentence boundaries

        Args:
            text: Text to split
            chunk_size: Maximum characters per chunk

        Returns:
            List of text chunks
        """
        if len(text) <= chunk_size:
            return [text]

        chunks = []
        current_chunk = ""

        # Split by sentences (Chinese and English)
        sentence_endings = ['。', '！', '？', '.', '!', '?']

        for char in text:
            current_chunk += char

            if char in sentence_endings and len(current_chunk) >= chunk_size * 0.8:
                if len(current_chunk) >= chunk_size * 0.5:
                    chunks.append(current_chunk.strip())
                    current_chunk = ""

        if current_chunk.strip():
            chunks.append(current_chunk.strip())

        return chunks

    async def close(self) -> None:
        """Close the HTTP client"""
        await self._client.aclose()
        logger.info("TextOptimizer client closed")

    async def __aenter__(self) -> "TextOptimizer":
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        await self.close()
