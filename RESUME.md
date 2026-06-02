# Codex 增强 #1 会话标题 + #4 生产力面板 接力文档

**起点 commit**: `a04a1ed` (main, 已 push 到 `fork` = git@github.com:LucaXiang/CCMeter.git)
**SPEC**: 无正式 spec；功能清单见 `CLAUDE.md` 末尾 "Possible future features"
**当前状态**: 工作树干净，全部已 commit。本会话已交付 fresh-input、per-model 拆分、Codex 面板、Codex 5h/7d 限流监控、限流命中、价值倍数、cwd-collision 修复、CLAUDE.md。剩 #1 #4 未做。

## 已做（本轮相关地基）

1. Codex 已折进 `EventIndex`：`src/data/index.rs:~207 fold_codex()`，按 (cwd,model,date,minute) 聚合，root 强制 `CODEX_ROOT`。
2. Codex 解析：`src/data/codex/parser.rs` 产 `CodexDelta{cwd,date,minute,model,input(fresh),cache_read,output}` —— **无 session_id**（#1 要加）。
3. Codex 面板：`src/ui/cards/render.rs:render_codex_breakdown()` + `src/app.rs CodexBreakdown{rows,daily}`（仅按模型，无按 session）。
4. Codex 限流：`src/data/codex/rate.rs`（rate_limits 解析 + `discover_codex_rate_limit_hits()`）。
5. 已探明数据源（都在 `~/.codex`）：`session_index.jsonl`{id,thread_name,updated_at}；`goals_1.sqlite::thread_goals`{thread_id,goal_text,tokens,start/end_ts}；会话 JSONL 内 `function_call` 事件含工具名(exec_command/apply_patch/write_stdin)。

## 待做

### #1 Codex 会话标题/目标（需新增 session 维度 + 列表面板）
- `src/data/codex/mod.rs` `struct CodexDelta` — 加 `pub session_id: String`（rollout 文件名里的 UUID，即 `session_index.jsonl` 的 `id`）。
- `src/data/codex/parser.rs:parse_codex_file()` 已知文件名 → 传 session_id 到 `parse_codex_str`，写入每个 CodexDelta。
- 新增 `src/data/codex/rate.rs` 同级模块（或 mod.rs）：读 `session_index.jsonl` → `HashMap<uuid, thread_name>`；可选读 `goals_1.sqlite::thread_goals`（用 `sqlite3` 或 rusqlite，注意 460MB 的是 logs_2.sqlite 不是这个）。
- 新聚合：每 session 的 (tokens, cost, last_date, thread_name) → `Vec<CodexSession>`，存进 `CodexBreakdown`（或新结构）。
- UI：`render_codex_breakdown` 下方加 "Recent Codex sessions" 列表（thread_name + tokens + cost + date），复用 `format_tokens`/`format_cost`。

### #4 工具/Git 生产力面板（新解析器 + 新面板）
- 新 `src/data/codex/activity.rs`：遍历 codex 会话，统计每日/每会话 tool calls(exec_command/apply_patch/write_stdin)、tool 失败、git commits、+/- 行。事件形状：会话 JSONL 里 `payload.type=="function_call"`（name=工具名）；git 信息可能在 exec 输出里（需验证字段，先 `rtk proxy rg '"function_call"' <一个session> | head` 看结构）。
- 注意性能：archived_sessions 共 1.6GB —— 只扫 `sessions/`（137MB，近期）或加缓存。
- UI：新面板或新 KPI（tool calls/day、accept 率、commits）。

## 必跑 (验证 gate)

```bash
cd /Users/xzy/workspace/CCMeter
cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning'   # 期望: 空（0 warning）
cargo +1.95.0 test 2>&1 | rg 'test result'                       # 期望: 78 passed; 2 failed（见踩坑）
cargo +1.95.0 clippy --bin ccmeter 2>&1 | rg 'generated'         # 期望: 11 warnings（既有 baseline，勿增）
```

## 踩过的坑 / 不能做的

- **构建**: `stable` toolchain 无 cargo，必须 `cargo +1.95.0`。是 **binary crate**（无 lib）：用 `cargo test` 不能 `--lib`。
- **替换二进制**: `cp` 到临时再 `mv -f`（原子）。直接 `cp` 覆盖正在启动的二进制 → `SIGKILL(9)`（不是代码 bug）。装到 `~/.cargo/bin/ccmeter`，target 在 `~/.cargo-target/release/`。
- **2 个既有失败测试** `rate_limits::detects_rate_limit_hit` / `::deduplicates_same_minute` —— 干净 HEAD 也失败（date-relative），**不是回归**，别去"修"。
- **grep 输出**: Bash 默认走 RTK 会压缩/改名（`PerModelUsage`→`nUsage`）。要精确输出用 `rtk proxy rg …`。
- **cwd-collision**: Codex 与 Claude 共享 cwd 字符串。index 聚合(`build_model_stats`)对 codex 条目**强制按 `CODEX_ROOT` 分组**，否则泄漏到 Claude 视图。新增 session 聚合同理：codex 数据只在 codex 源下展示。
- **成本口径**: `input` 存 fresh(去缓存)；`cost_from_tokens` 要 cache-inclusive，内部再减。Codex cost 74% 是缓存重读，是 API 等价估算非真实账单。
- **push**: `git push fork main`（origin=hmenzagh 上游不可推）。

## 下一 session 第一句

> 读 `/Users/xzy/workspace/CCMeter/RESUME.md` 和 `CLAUDE.md`，然后实现 #1 Codex 会话标题/目标：给 `CodexDelta` 加 `session_id`，按 session 聚合 tokens/cost，读 `~/.codex/session_index.jsonl` 拿 thread_name，在 Codex 源视图的 per-model 面板下方加一个 "Recent Codex sessions" 列表（标题+tokens+成本+日期）。TDD，原子提交，`cargo +1.95.0` 构建，完成后原子替换 `~/.cargo/bin/ccmeter` 并 `git push fork main`。
