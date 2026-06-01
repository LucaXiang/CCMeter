# CCMeter 多源 · 全历史 · 多 Provider 设计

**日期:** 2026-06-01
**状态:** 待审 (brainstorming 产出,实现前需 review)
**作者:** xzy + Claude

---

## 1. 背景与问题

CCMeter 目前实际上只展示**最近约 30 天**的 Claude Code 用量。经过对源码与本机数据源的完整排查,根因和可恢复性如下:

- CCMeter 代码层**没有** 30 天硬限制:`TimeFilter::All` 范围是 `NaiveDate::MIN..MAX`(`src/ui/time_filter.rs:122`),默认过滤器就是 `All`(`src/app.rs:208`),持久化缓存 `~/.config/ccmeter/history.json` 用 high-water-mark merge **永不裁剪**(`src/data/cache.rs:353`)。
- 真正的限制来自 **Claude Code 自身**:它默认 30 天清理会话转录(`cleanupPeriodDays`),CCMeter 只能读到现存的 `~/.claude/projects/**/*.jsonl`。
- 但**历史并未彻底丢失**。本机存在多个持久化数据源,token 历史最早可回溯到 **2026-01-01**。

参考实现:[slopmeter](https://github.com/JeanMeijer/slopmeter)(`packages/cli/src/lib/claude-code.ts`、`codex.ts`)用 Claude 自有文件(JSONL + `stats-cache.json` + `history.jsonl`)就画出了回到 1 月 1 日的热力图,并支持 Codex 等多 provider。本设计借鉴其分层与多源思路,落到 CCMeter 的 Rust 架构上。

### 本机已核实的数据源清单

**Claude Code**

| 源 | 路径 | 跨度 | 粒度 |
|---|---|---|---|
| 原始 JSONL | `~/.claude/projects/**/*.jsonl` | 5/3 → 今 (~30天) | 全保真:逐消息 token、成本、cwd/项目、代码行、模型、时间戳 |
| stats-cache.json | `~/.claude/stats-cache.json` → `dailyModelTokens[]` | **1/1 → 4/16** | 每日 × 模型的 token **总量**;无项目、无代码行、无成本拆分。另有 `dailyActivity`、`modelUsage`、`firstSessionDate` |
| Code Insights 库 | `~/.code-insights/data.db` (SQLite, ~1GB) | 3/25 → 今 | 富:`messages.usage`(逐消息 token/模型/成本)、`sessions`(逐会话聚合 + `project_path`=cwd)。**无 `structuredPatch`**(无接受行数);`tool_calls` 含 Edit/Write(可得建议行数) |
| claude-mem | `~/.claude-mem/claude-mem.db` | 2/28 → 今 | 仅语义摘要/提问;**无 token/成本**。本设计不使用 |

**Codex**

| 源 | 路径 | 跨度 | 粒度 |
|---|---|---|---|
| Codex sessions | `~/.codex/sessions/**/*.jsonl` | 5/2 → 今 (223 文件) | `event_msg`/`token_count` 记录带 `last_token_usage`/`total_token_usage`(累计:input/cached/output/reasoning/total),模型来自 `turn_context` |

### 关键事实校验

- `stats-cache.json` `dailyModelTokens` 97 条,范围 `2026-01-01 → 2026-04-16`;首条即 1/1。4/16 后停更(新版 Claude Code 改用 `usage-data/` 机制)。
- Code Insights `messages.usage` 为**逐消息增量**(单会话逐条求和 ≈ `sessions.total_*`,output 误差 <1%),`project_path` 即 cwd 格式,38 个去重项目。
- Time Machine 未配置;1 月之前的 token 数据任何源都没有(`claudeCodeFirstTokenDate = 2025-11-23` 仅为标记,无每日明细)。

---

## 2. 目标与非目标

### 目标(用户诉求,分三块)

- **A. Claude Code 全历史回填** —— 让 CCMeter 展示回溯至 1/1 的历史,而非仅 30 天。
- **B. Codex 支持** —— 解析 `~/.codex/sessions/`,接入 CCMeter 用量统计。
- **C. 合并统计** —— Claude Code 与 Codex 一起统计,UI 可区分/合计。

### 推进方式

**分阶段:A → B → C。** 每阶段独立可验证、独立合并。本 spec 详述 A,概述 B/C(各自后续单独出 plan)。

### 非目标

- 不恢复 1 月之前的数据(物理上不存在)。
- 不引入云同步/上传;纯本地。
- 不修改 Claude Code 的任何文件或设置(只读)。
- 不追求老日子的逐项目/逐行/逐模型完全保真——粗粒度可接受,但必须**诚实标注**。

---

## 3. Phase A:Claude Code 全历史回填(详细设计)

### 3.1 分层取最全(核心原则)

"取最全"= **每个日期用能覆盖它的最高保真源,日期范围互不重叠**,而非跨源相加(相加会重复计数)。

```
boundary_jsonl = 当前 JSONL 解析覆盖到的最早本地日期(动态)
boundary_ci    = Code Insights 覆盖到的最早日期 (~3/25)

日期 ≥ boundary_jsonl            → 原始 JSONL(现有实时管线,不改)
boundary_ci ≤ 日期 < boundary_jsonl → Code Insights 回填
1/1 ≤ 日期 < boundary_ci          → stats-cache 回填
```

回填**只产出 `< boundary_jsonl` 的日期条目**,因此与实时数据天然不重叠。回填内部:同一天若 CI 和 stats-cache 都有,CI 优先。

### 3.2 落到 CCMeter 缓存模型

CCMeter 缓存键:`(source_root, cwd, date) → DayEntry{input, output, cache_read, cache_creation, cost, lines_suggested, lines_accepted, lines_added, lines_deleted, active_minutes}`(`src/data/cache.rs:14`)。全局聚合 `to_daily_tokens_filtered(None, None)` 跨所有 root/cwd 求和 → 喂给热力图/KPI。

回填条目写入持久化缓存,使用**专用合成 source_root** 以便识别与按源过滤:

- **Code Insights 回填**
  - `source_root = "backfill:code-insights"`(合成标记)
  - `cwd = sessions.project_path`(真实 cwd → 现有 git 分组可识别)
  - 按 `messages.timestamp` 本地日期分桶;token 取 `messages.usage` 的 input/output/cacheRead/cacheCreation
  - `cost` 由 token 经 CCMeter 定价(`src/data/models.rs::model_pricing`)**重算**
  - `lines_suggested/added/deleted` 由 `tool_calls` 中 Edit/Write 的 old/new_string 走 `count_diff_lines` 复原;`lines_accepted` 无源 → 0(标注)
  - `active_minutes` 由该 (cwd, date) 的消息时间戳走现有 `cluster_active_minutes`(`src/data/cache.rs:341`)
- **stats-cache 回填**
  - `source_root = "backfill:stats-cache"`
  - 无项目维度 → `cwd = "(historical)"` 单一伪项目(每个 date 一条)
  - `tokensByModel` 仅给每模型 token 总量;input/output/cache 拆分用 slopmeter 同款方法:按全局 `modelUsage` 比例 `distributeTokenComponents` 还原(`claude-code.ts:187/223`)
  - `cost` 由还原后的 token 经 CCMeter 定价重算
  - `lines_* = 0`、`active_minutes` 取 `dailyActivity` 若有,否则 0(标注)

> **去重保证**:回填仅覆盖 `< boundary_jsonl` 的日期,且每个源用不同合成 `source_root`,内部按日期分段不重叠 → `to_daily_tokens_filtered(None,None)` 求和不会重复计入。

### 3.3 已知保真度限制(必须在 UI 标注)

1. **老日子无逐项目细分**:stats-cache 段(1/1–~3/24)仅进总量/热力图/成本,不出现在按项目卡片(其 cwd 为 `(historical)`)。
2. **缓存无模型维度**:`DayEntry` 无 model 字段,模型分布来自只读实时 `EventIndex`。回填历史有 token/成本但**无逐模型细分**。→ 可选子任务:扩展缓存 schema 增加 per-model(见 §6)。
3. **`lines_accepted` 缺失**:CI 段无 `structuredPatch`;stats-cache 段无任何行数。
4. **成本为重算估算**,非账单原值。

UI 处理:在热力图/KPI 下方加一行注脚,动态显示 "Full fidelity since {boundary_jsonl}; {boundary_ci}–{boundary_jsonl} from Code Insights; {1/1}–{boundary_ci} token-only (project/line breakdown unavailable)."。仿 slopmeter "earlier activity may be undercounted"。

### 3.4 触发机制

- 新增 CLI 子命令 **`ccmeter backfill`**(clap derive,`src/main.rs`):
  - 读三源 → 按 §3.1 分层 → 构造合成缓存 → `cache::merge` 进 `history.json` → `cache::save`
  - **幂等**:重复跑结果一致(合成 root 固定、max-merge 收敛、按日期分段确定)
  - flags:`--source <all|stats-cache|code-insights>`、`--dry-run`(只打印将写入的日期范围与汇总,不落盘)、`--since <YYYY-MM-DD>`
- 首次启动检测:若检测到可回填的历史源且缓存最早日期晚于源最早日期,显示一次性提示 "Run `ccmeter backfill` to import history back to {date}"(不自动写盘,尊重用户)。

### 3.5 新增依赖

- `rusqlite`(读 Code Insights `data.db`),启用 `bundled` feature 避免系统 SQLite 依赖。仅 `backfill` 路径使用,考虑置于 feature gate 以免增大常驻 TUI 体积(评估后定)。

### 3.6 模块落点(新增,隔离清晰)

```
src/data/backfill/mod.rs          # 编排:分层 + 合并入口 run_backfill()
src/data/backfill/stats_cache.rs  # 解析 ~/.claude/stats-cache.json → Vec<(root,cwd,date,DayEntry)>
src/data/backfill/code_insights.rs# 读 ~/.code-insights/data.db → 同上
src/data/backfill/layering.rs     # 按日期 boundary 取最全、去重
```

每个文件单一职责、可独立单测(给定假数据库/假 JSON → 断言产出的 DayEntry)。

### 3.7 测试策略

- `stats_cache.rs`:喂构造的 `dailyModelTokens` JSON,断言日期分桶、模型比例拆分、定价重算。
- `code_insights.rs`:用 `rusqlite` 建内存库填 `sessions`/`messages`,断言逐消息增量求和、cwd 映射、Edit/Write 行数复原、`lines_accepted=0`。
- `layering.rs`:三源给重叠日期,断言每日期只被最富源覆盖、无重复计数(总量 = 各段之和)。
- 端到端:`backfill --dry-run` 在真实只读源上跑,断言不 panic、日期范围合理、不写盘。

---

## 4. Phase B:Codex 支持(概要)

- 新 provider 解析器 `src/data/providers/codex.rs`:扫 `~/.codex/sessions/**/*.jsonl`,取 `event_msg`/`token_count` 的 `total_token_usage`(累计)做增量(CCMeter 已有 deltaize 逻辑可复用思路),模型取 `turn_context`。
- Codex token 维度:input / cached_input / output / reasoning_output。映射到 `DayEntry`:`input += input`、`cache_read += cached_input`、`output += output + reasoning_output`(对齐现有语义,待定)。成本按 Codex/OpenAI 定价表(新增)。
- cwd:Codex 会话记录含工作目录(待核实字段),用于项目分组;否则归入 `(codex)` 伪项目。
- 需要引入 **provider 维度**:见 Phase C。

## 5. Phase C:合并统计(概要)

- 在缓存或聚合层引入 `provider ∈ {claude-code, codex}` 维度。最小改动方案:把 provider 编码进 `source_root`(如 `codex:~/.codex`),复用现有 source 过滤 UI(`src/app.rs:build_source_list`)做 provider 切换;默认 "All" 合计。
- UI:source 选择器增加 "Claude Code / Codex / All";KPI/热力图按所选 provider 过滤或合计。
- 风险:成本口径跨 provider 不同(定价表不同);需分别定价后再合计。

---

## 6. 可选增强(YAGNI,先不做)

- 扩展缓存 schema 增加 per-(date,model) token,使回填历史也能展示模型分布(需 schema bump → `CURRENT_SCHEMA_VERSION` +1,`src/data/cache.rs:49`)。
- 读 `~/.claude/usage-data/`(4/16 后新机制)补 stats-cache 停更后的空档(若 CI/JSONL 未覆盖)。
- `usage-data/report.html` 解析(不建议;HTML 易碎)。

---

## 7. 风险与权衡

| 风险 | 缓解 |
|---|---|
| 跨源重复计数 | 按日期 boundary 严格分段;每源独立合成 root;dry-run 断言总量守恒 |
| 老数据被误当全保真 | UI 注脚 + 合成 cwd `(historical)` 明确标注 |
| rusqlite 增大体积/编译 | feature gate;仅 backfill 用 |
| Code Insights 是第三方、schema 可能变 | 解析容错(缺列/缺表则跳过该源,降级到 stats-cache);版本探测 |
| 边界日期漂移导致重复 | boundary 取实时 JSONL 实际最早日期,回填严格 `<` 该日期 |

---

## 8. 验收标准(Phase A)

- `ccmeter backfill` 跑完后,`history.json` 含 `2026-01-01` 起的日期条目。
- TUI `All` 视图热力图/总成本/总 token 覆盖到 1 月,且**与分段求和一致**(无重复计数,可用 dry-run 核对)。
- 5/3 起的日子仍为全保真(项目卡片、行数不变)。
- 老日子在 UI 上有明确的"粗粒度/未细分"标注。
- 全部新增模块有单测;`cargo test` 通过;`cargo clippy` 无新增告警。
