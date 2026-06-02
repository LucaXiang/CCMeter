# CCMeter 接力文档 — 统一项目卡(Claude + Codex)

**当前 HEAD**: `1cf2c7d` (已 merge 进 `main` 并 push 到 `fork` = git@github.com:LucaXiang/CCMeter.git)
**SPEC**: `docs/superpowers/specs/2026-06-02-unified-project-cards-design.md`
**计划**: `docs/superpowers/plans/`(unified-project-cards-phase{1,2,3} + ui-layout-polish-phase4 + i18n-phase5)
**状态**: 统一项目卡(P1–3)+ UI 布局打磨(P4)+ i18n 中英双语(P5)+ Codex 凭证 bug **全部完成**。仅剩 #4 生产力面板(独立特性)。
**测试基线**: `cargo +1.95.0 test` → 107 passed; 2 failed(既有 date-relative rate_limits);clippy 11。
**最近增量(P4/P5/bug)**:
- 🔴 Codex 合成凭证每 5min discovery 刷新被丢 → 幂等 `with_codex_credential`,启动+刷新两路径都附上(`oauth.rs`/`app.rs spawn_discovery`)。
- 🟣 P4 布局:卡片高 8 每组一行;明细 Cost/Tokens 图表纵向堆叠全宽;Recent sessions ↑↓ 滚动(`App.detail_session_scroll`,仅 `project_index.is_some()` 时)。
- 🟣 P5 i18n:`src/ui/i18n.rs`(`Lang`/`detect`/全局 `AtomicU8`/`t(&'static str)`+ ~180 条 ZH 表,未命中回退英文);全 UI 文案包 `t(...)`——**注意 `let t = theme()` 遮蔽 → 用全限定 `crate::ui::i18n::t`**;`Settings.language` + 启动 `$LANG`/`$CCMETER_LANG` 检测 + Display 标签 Language 开关(实时切+持久化)。加新文案就往 `zh()` 加一条 `"en" => "中文"`(键=代码里**逐字**含空格的字面量)。

## 已完成 — Phase 1：统一分组 + 管线

把 Codex 用量归进**按仓库的项目卡片**(worktree 用会话里存的 git `repository_url` 收敛),与 Claude 同仓库合卡。真机验证:`workspace/crab` 一张卡 = 7 Claude 源 + 28 Codex 源(35 cwd),按 `github.com/lucaxiang/crab` 合并。

地基(全部已提交、已测试、已审查):
1. `src/config/identity.rs`(新):`normalize_remote_url`、`strip_worktree_segment`、`IdentityStore`(持久化 sidecar `~/.config/ccmeter/identities.json`,独立 schema v1)、`resolve`(**live-git 优先 → 持久化 → 路径兜底**)、`seed`(Codex URL 反哺,规范化后存)。
2. `src/data/codex/sessions.rs`(新):`parse_session_meta` / `collect_codex_session_meta` —— 从 `session_meta` 取 `(session_id, cwd, repository_url, repo_root)`。
3. `src/config/discovery.rs`:`Provider{Claude,Codex}` + `ProjectSource.provider`;`discover_project_groups_unified()`(Claude+Codex 单管线:收集元数据→seed→resolve→`group_with_store` 合卡);`finalize_groups`/`build_root_and_session_maps` 共享抽取;**遗留 Claude-only 路径已删**(`group_by_identity`/`resolve_identity`/`heuristic_root` 等)。
4. `src/data/index.rs`:去掉 `build_model_stats` 的 `CODEX_ROOT` 特判 → Codex 按模型归到仓库组(源 tab 仍由 `entry_passes` 的 RootFilter 隔离)。
5. `src/app.rs`/`dashboard.rs`/`cards/render.rs`:`App::new`+`spawn_discovery` 用统一 discovery;**退役 `CodexBreakdown` 专用面板**(Codex 模型走每卡 model 拆分)。

**关键不变量**:cache/index 里 Codex 仍挂 `CODEX_ROOT`(= provider 维度,源 tab 靠它筛),**无日 cache schema bump**;卡片按 group 的 cwd 集合 `iter_filtered` 取数,同仓库 Claude+Codex 自然合并。

## 已完成 — Phase 2：卡面 provider 拆分

计划 `docs/superpowers/plans/2026-06-02-unified-project-cards-phase2.md`。`ProjectCard` 加 `cost_claude/cost_codex`,`build_cards` 循环按 `root == CODEX_ROOT` 拆(两者和 = total_cost);`render_card` line 1 成本后渲染 ` cc $X · cx $Y`(仅 mixed 卡显示,宽度守卫,padding 两处已补 `split_str.len()`)。`ProjectSource.provider` 的 `#[allow(dead_code)]` 仍在(Phase 2 读的是 cache root,不是该字段;Phase 3 若需要再用)。

## 已完成 — Phase 3：Recent sessions(含标题)

计划 `docs/superpowers/plans/2026-06-02-unified-project-cards-phase3.md`。
- `src/data/sessions.rs`(新):`SessionSummary{title,provider,cwd,tokens,cost,last_date}`、`claude_session_summaries`(从 events 按 session_file 聚合)、`scan_ai_titles`(扫 `{"type":"ai-title","aiTitle":..}`)、`fit_cols`(CJK 显示宽度截断,中文标题 2 列对齐)。
- `CodexDelta` 加 `session_id`(`parse_codex_str` 从 `session_meta.payload.id` 盖章);`codex/sessions.rs` 加 `read_thread_names`(读 `session_index.jsonl`)+ `codex_session_summaries`(cost 口径同 cache/index)。
- `app.rs`:`load_data` 算全量 sessions 存 `AppData.sessions`(`ReloadResult` 同步带上);`build_render_cache` 按选中项目 `project_cwds`+日期过滤 → `RenderCache.detail_sessions`。
- `render_detail` 切出底部块 `render_recent_sessions`(高度守卫,charts 保 ≥4 行)列 `CC/CX · 标题 · tokens · 成本 · 日期`,按 last_date 倒序。
- 真机验证:crab 420 个 Codex 会话,248 个解析出真实中文 `thread_name`;Claude 283/300 文件有 `ai-title`。

## 待做

### #4 工具/Git 生产力面板(独立,见 CLAUDE.md 末)
新解析器统计每会话/每日 tool calls(exec_command/apply_patch/write_stdin)、失败、git commits、+/- 行;新面板或 KPI。注意性能(archived_sessions 体量大)。

## 必跑 (验证 gate)
```bash
cd /Users/xzy/workspace/CCMeter
cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'   # 期望: 空(0 warning)
cargo +1.95.0 test 2>&1 | rg 'test result'                       # 104 passed; 2 failed(见踩坑)
cargo +1.95.0 clippy --bin ccmeter 2>&1 | rg 'generated'         # 11 warnings(baseline,勿增)
```

## 踩过的坑 / 不能做的
- **构建**: `stable` 无 cargo,必须 `cargo +1.95.0`。binary crate(无 lib),用 `cargo test` 不能 `--lib`。
- **替换二进制**: `cp` 到临时再 `mv -f`(原子)。target 在 `~/.cargo-target/release/`,装到 `~/.cargo/bin/ccmeter`。
- **2 个既有失败测试** `rate_limits::detects_rate_limit_hit` / `::deduplicates_same_minute` —— 干净 HEAD 也失败(date-relative),**不是回归**。
- **headless 跑二进制** 会报 `Error: Device not configured`(无 TTY,crossterm 终端初始化失败)—— 不是 bug,真实终端正常。
- **grep 输出**: Bash 默认走 RTK 会压缩/改名。要精确输出用 `rtk proxy rg …`。
- **分组碰撞坑(已修)**: `group_with_store` 适配到 root-keyed map 时必须**按 root 合并**(extend sources),不能 `.collect()`(会丢同 root 的另一组 —— 真机上 crab 的已删 worktree 组就会被丢)。
- **push**: `git push fork main`(origin=hmenzagh 上游不可推)。

## 下一 session 第一句
> 统一项目卡(Phase 1–3)已完成。若做 #4 生产力面板:新 `src/data/codex/activity.rs` 遍历 codex 会话统计 `payload.type=="function_call"`(name=工具名 exec_command/apply_patch/write_stdin)、失败、git commits、+/- 行(git 信息可能在 exec 输出里,先 `rtk proxy rg '"function_call"' <一个session> | head` 看结构);只扫 `sessions/` 或加缓存(archived 体量大)。先 writing-plans 再子代理 TDD,`cargo +1.95.0`,完成后原子替换二进制并 `git push fork main`。
