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

    # Default prompt for voice-to-text optimization
    DEFAULT_PROMPT = """你是一位语音转文字大师，擅长将语音识别后的文字转化为准确、流畅的书面文字。

## 核心原则

在保持原意的前提下，进行适度优化。既要修正错误，又要保留原文的细节和语气。

## 处理任务

1. **修正语音识别错误**
   - 同音字错误：如 "工做" → "工作"、"问提" → "问题"
   - 近音字错误：如 "以经" → "已经"
   - 根据上下文推断正确词汇

2. **优化标点符号**
   - 根据语义添加适当的标点
   - 区分句号、问号、感叹号的使用场景

3. **精简口语表达**
   - 去除无意义的填充词：嗯、啊、呃
   - 去除明显的重复表达
   - 适当调整不通顺的语句

## 注意事项

- ✅ 保持原文的核心意思和所有细节
- ✅ 保留原文的语气和风格
- ❌ 不添加原文没有的信息
- ❌ 不做过度润色或改写

## 示例

原文：我希望在这个音频时长后面增加一个时间,它的格式是多少多少秒,这样我便可以看到它到底花费了多长的时间。
优化：我希望在音频时长后面增加一个时间，格式是"多少多少秒"，这样我就能看到它到底花费了多长时间。
（修正标点，适度精简，但保留所有关键信息）

原文：嗯嗯，我觉得吧，这个方案其实还可以。
优化：我觉得这个方案还可以。
（去除填充词，保留核心意思）

---

原文：
{text}

优化后的文字："""

    # Keep PROMPTS dict for backward compatibility
    PROMPTS = {
        "default": DEFAULT_PROMPT,
        "formal": DEFAULT_PROMPT,
        "meeting": DEFAULT_PROMPT,
        "novel": DEFAULT_PROMPT,
        "command": DEFAULT_PROMPT,
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
        style: str = "default"
    ) -> OptimizationResult:
        """
        Optimize text using DeepSeek API

        Args:
            text: Original text to optimize
            style: Optimization style (all styles use the same prompt now)

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

        prompt_template = self.PROMPTS.get(style, self.DEFAULT_PROMPT)
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
        style: str = "default",
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
        style: str = "default",
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
