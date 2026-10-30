# CLAUDE.md

本项目的 AI 编码指引**以 [AGENTS.md](AGENTS.md) 为准**，请先完整读它；两者冲突时以 AGENTS.md 为准并顺手修正本文。

## Claude Code 特有的注意点

1. **剩余工作看 `docs/Tauri重写-04-交接-剩余移植与Python下线.md`**：本地 Whisper / Qwen 引擎尚未移植，
   `app/`（Python）与 `electron/` 是待下线的旧代码，不要在里面加功能。
2. **不要用 `git add -A`**：仓库里有未纳入版本库的审计截图与设计产物（`docs/ui-audit/`、`.tmp-shots/`、`.stitch/`）。改动按文件显式暂存。
3. **提交信息用中文 Conventional Commits**：`<type>(<scope>): <中文描述>`，一个提交只做一件逻辑完整的事。
4. **改完必须跑**：`cargo test --workspace --manifest-path src-tauri/Cargo.toml` 与 `node scripts/run-node-tests.mjs`；
   前端有改动再跑 `npm run build`。
5. **正在运行的 `tauri dev` 会自动重编译并读写真实 `data/`**：涉及数据迁移/格式的改动先备份 `data/`。
6. **不要修改 `.env`**（含真实密钥，已被 `.gitignore` 忽略）。
