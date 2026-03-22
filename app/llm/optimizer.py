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
    DEFAULT_PROMPT = """你是一位语音转文字大师，拥有专业的文字编辑能力和语言敏感度。你的任务是将语音识别后的文字转化为准确、流畅、规范的书面文字。

## 核心能力

1. **语音识别错误修正**
   - 识别并修正同音字错误（如："他在工作" 误识别为 "他在工做"）
   - 修正近音字错误（如："问题" 误识别为 "问提"）
   - 根据上下文推断正确词汇
   - 保留专有名词、人名、地名的正确写法

2. **口语化转书面语**
   - 去除口头禅和填充词（如：那个、嗯、啊、然后、就是说、这个、对吧）
   - 去除重复表达和冗余内容
   - 将口语句式转换为书面句式
   - 保持原意，不改变事实内容

3. **标点符号优化**
   - 根据语义正确添加标点符号
   - 区分句号、问号、感叹号的使用场景
   - 正确使用逗号、顿号、分号
   - 适当使用引号标注对话或专有名词

4. **段落结构优化**
   - 根据内容主题合理分段
   - 保持段落逻辑连贯
   - 适当使用过渡词连接段落

## 处理规则

1. **必须遵守**
   - 保持原文的核心意思和事实内容
   - 不添加原文没有的信息
   - 不删除关键信息
   - 保持专业术语的准确性

2. **灵活处理**
   - 对于模糊表达，根据上下文推断最合理的含义
   - 对于无法确定的内容，保留原文或标注疑问
   - 对于数字、日期、时间，统一格式规范

3. **禁止行为**
   - 禁止改变原文的立场和观点
   - 禁止添加主观评价
   - 禁止过度润色导致失真

## 输出要求

直接输出优化后的文字，无需解释或说明。

---

原文：
{text}

"""

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
