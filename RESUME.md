# CCMeter 接力文档 — 统一项目卡(Claude + Codex)

**当前 HEAD**: `074ce8a` (已 merge 进 `main` 并 push 到 `fork` = git@github.com:LucaXiang/CCMeter.git)
**SPEC**: `docs/superpowers/specs/2026-06-02-unified-project-cards-design.md`
**计划**: `docs/superpowers/plans/2026-06-02-unified-project-cards-phase1.md`

## 已完成 — Phase 1：统一分组 + 管线

把 Codex 用量归进**按仓库的项目卡片**(worktree 用会话里存的 git `repository_url` 收敛),与 Claude 同仓库合卡。真机验证:`workspace/crab` 一张卡 = 7 Claude 源 + 28 Codex 源(35 cwd),按 `github.com/lucaxiang/crab` 合并。

地基(全部已提交、已测试、已审查):
1. `src/config/identity.rs`(新):`normalize_remote_url`、`strip_worktree_segment`、`IdentityStore`(持久化 sidecar `~/.config/ccmeter/identities.json`,独立 schema v1)、`resolve`(**live-git 优先 → 持久化 → 路径兜底**)、`seed`(Codex URL 反哺,规范化后存)。
2. `src/data/codex/sessions.rs`(新):`parse_session_meta` / `collect_codex_session_meta` —— 从 `session_meta` 取 `(session_id, cwd, repository_url, repo_root)`。
3. `src/config/discovery.rs`:`Provider{Claude,Codex}` + `ProjectSource.provider`;`discover_project_groups_unified()`(Claude+Codex 单管线:收集元数据→seed→resolve→`group_with_store` 合卡);`finalize_groups`/`build_root_and_session_maps` 共享抽取;**遗留 Claude-only 路径已删**(`group_by_identity`/`resolve_identity`/`heuristic_root` 等)。
4. `src/data/index.rs`:去掉 `build_model_stats` 的 `CODEX_ROOT` 特判 → Codex 按模型归到仓库组(源 tab 仍由 `entry_passes` 的 RootFilter 隔离)。
5. `src/app.rs`/`dashboard.rs`/`cards/render.rs`:`App::new`+`spawn_discovery` 用统一 discovery;**退役 `CodexBreakdown` 专用面板**(Codex 模型走每卡 model 拆分)。

**关键不变量**:cache/index 里 Codex 仍挂 `CODEX_ROOT`(= provider 维度,源 tab 靠它筛),**无日 cache schema bump**;卡片按 group 的 cwd 集合 `iter_filtered` 取数,同仓库 Claude+Codex 自然合并。

## 待做

### Phase 2：卡面 provider 拆分
`build_cards`(`src/ui/cards/data.rs`)累加时按 `root == CODEX_ROOT` 拆 `(claude, codex)`;`ProjectCard` 加 `cost_claude/cost_codex`;卡面渲染迷你拆分(如 `CC $30 · CX $12`)。`ProjectSource.provider` 字段已就位(现 `#[allow(dead_code)]`,Phase 2 的读取者)。

### Phase 3：Recent sessions(含标题)= 原 #1 泛化到两端
- Codex:`CodexDelta` 加 `session_id`,按 session 聚合 tokens/cost/last_date;`thread_name` 取 `~/.codex/session_index.jsonl`。
- Claude:按 session 文件聚合;标题取会话 JSONL 的 `ai-title`(`{"type":"ai-title","aiTitle":..}`),回退首条 prompt / cwd basename。
- 经 cwd→group 归到卡;卡片**明细视图**列 `标题·tokens·成本·日期·来源(CC/CX)`,按最近活动倒序。

### #4 工具/Git 生产力面板(独立,见 CLAUDE.md 末)

## 必跑 (验证 gate)
```bash
cd /Users/xzy/workspace/CCMeter
cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'   # 期望: 空(0 warning)
cargo +1.95.0 test 2>&1 | rg 'test result'                       # 95 passed; 2 failed(见踩坑)
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
> 读 `RESUME.md` 和 spec/plan,实现 Phase 2(卡面 provider 拆分):`build_cards` 按 `CODEX_ROOT` 拆 `cost_claude/cost_codex`,`ProjectCard` 加字段,卡面渲染迷你拆分条。TDD,`cargo +1.95.0`,完成后原子替换二进制并 `git push fork main`。
