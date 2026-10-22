# CLAUDE.md

本项目的 AI 编码指引**以 [AGENTS.md](AGENTS.md) 为准**，请先完整读它。

这里只补充 Claude Code 使用中容易踩的几点，避免两份文件内容重复后各自腐化：

## 与 AGENTS.md 的关系

- 架构、命令、硬约束、陷阱清单、已知缺陷、常见任务做法：全部在 `AGENTS.md`，本文不再复制。
- 若发现两份文件冲突，**以 `AGENTS.md` 为准**，并顺手修掉本文的过期表述。

## Claude Code 特有的注意点

1. **改动前先看 `docs/已知问题.md`**：那里列着当前确认的缺陷（`--win` 打包
   在 nsis 阶段失败、多段录音上传失败会卡住）。别在不了解现状的情况下
   "顺手重构"这些区域。LLM provider 无超时已于 2026-10-22 随 LLM v2 重写修复
   （`docs/LLM配置模块重设计.md`）。
2. **不要用 `git add -A`**：仓库里有未纳入版本库的审计截图与设计产物
   （`docs/ui-audit/`、`.tmp-shots/`、`.stitch/`，已在 `.gitignore` 里）。改动请按文件显式暂存。
3. **提交信息用中文 Conventional Commits**：`<type>(<scope>): <中文描述>`，一个提交只做一件逻辑完整的事。
4. **改完必须跑**：`uv run python -m pytest tests -q` 与 `node scripts/run-node-tests.mjs`。
   前端有构建影响时再跑 `npm run build`。
5. **Windows 环境注意**：控制台默认 GBK，脚本里 print 中文/符号前先
   `sys.stdout.reconfigure(encoding="utf-8")`（参考 `scripts/probe_doubao_asr.py`）；
   `data/settings.json` 与 `.env` 同时存在时前者优先。
6. **不要修改 `.env`**（含真实密钥，已被 `.gitignore` 忽略）；需要新增变量时改 `.env.example`
   并在 `docs/配置说明.md` 里同步说明。

## 文档索引

`README.md`、`AGENTS.md`、`docs/使用说明.md`、`docs/架构说明.md`、`docs/配置说明.md`、
`docs/API.md`、`docs/ASR接入.md`、`docs/开发与运维.md`、`docs/已知问题.md`、
`docs/LLM配置模块重设计.md`（LLM v2 设计与原型）、`docs/文件转写-多引擎与LLM校对-设计.md`（文件转写多引擎与 LLM 校对设计）。
