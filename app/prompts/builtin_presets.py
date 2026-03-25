"""
Built-in Prompt Presets
Default prompt templates for common scenarios
"""

BUILTIN_PRESETS = {
    "general": {
        "id": "builtin-general",
        "name": "通用优化",
        "description": "适用于日常语音转文字的通用优化",
        "category": "general",
        "prompt_template": """你是一位语音转文字专家，请优化以下语音转写文本。

要求：
1. 修正语音识别错误
2. 添加正确的标点符号
3. 去除口语化表达和重复内容
4. 保持原意，使表达更加通顺

原文：
{text}

优化后的文字：""",
        "is_builtin": True
    },

    "coding": {
        "id": "builtin-coding",
        "name": "程序员模式",
        "description": "针对程序员场景，优化代码术语和英文表达",
        "category": "coding",
        "prompt_template": """你是程序员助手，请优化以下语音转写的技术内容。

要求：
1. 准确保留技术术语和英文单词
2. 修正代码相关的识别错误
3. 保持技术表达的准确性
4. 适当添加代码格式化标记

原文：
{text}

优化后的文字：""",
        "is_builtin": True
    },

    "novel": {
        "id": "builtin-novel",
        "name": "小说创作",
        "description": "针对小说创作场景，保持文学风格",
        "category": "novel",
        "prompt_template": """你是文学编辑，请优化以下语音转写的小说内容。

要求：
1. 保持人物对话的风格一致性
2. 统一人名和地名
3. 增强场景描写的文学性
4. 保留作者的写作风格

原文：
{text}

优化后的文字：""",
        "is_builtin": True
    },

    "meeting": {
        "id": "builtin-meeting",
        "name": "会议记录",
        "description": "整理会议内容，提取关键信息",
        "category": "meeting",
        "prompt_template": """你是会议记录助手，请整理以下语音转写的会议内容。

要求：
1. 提取关键讨论点和决议
2. 识别不同发言者（如有）
3. 整理成结构化的会议纪要
4. 突出行动项和待办事项

原文：
{text}

整理后的会议纪要：""",
        "is_builtin": True
    },

    "marketing": {
        "id": "builtin-marketing",
        "name": "文案优化",
        "description": "针对营销文案场景，增强感染力",
        "category": "marketing",
        "prompt_template": """你是营销文案专家，请优化以下语音转写的营销内容。

要求：
1. 增强文案的吸引力和说服力
2. 突出产品/服务的核心卖点
3. 保持品牌调性一致
4. 优化语言表达，更具感染力

原文：
{text}

优化后的文案：""",
        "is_builtin": True
    }
}
