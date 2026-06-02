use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Zh,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    LANG.store(if l == Lang::Zh { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 {
        Lang::Zh
    } else {
        Lang::En
    }
}

pub fn from_lang_str(s: &str) -> Lang {
    if s.to_lowercase().contains("zh") {
        Lang::Zh
    } else {
        Lang::En
    }
}

pub fn detect() -> Lang {
    for var in ["CCMETER_LANG", "LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return from_lang_str(&v);
            }
        }
    }
    Lang::En
}

pub fn t(en: &'static str) -> &'static str {
    match current() {
        Lang::En => en,
        Lang::Zh => zh(en),
    }
}

fn zh(en: &'static str) -> &'static str {
    match en {
        "1h" => "1小时",
        "12h" => "12小时",
        "Today" => "今天",
        "Last week" => "上周",
        "Last month" => "上个月",
        "All" => "全部",
        "All projects" => "所有项目",
        " Cost USD " => " 费用 USD ",
        " Streak " => " 连续天数 ",
        " Active days " => " 活跃天数 ",
        " Avg/day " => " 日均 ",
        " Total tokens " => " 总 Token 数 ",
        " Efficiency " => " 效率 ",
        " days" => " 天",
        " Input " => " 输入 ",
        " Output " => " 输出 ",
        " Lines " => " 行数 ",
        " Accept " => " 接受率 ",
        "Jan" => "1月",
        "Feb" => "2月",
        "Mar" => "3月",
        "Apr" => "4月",
        "May" => "5月",
        "Jun" => "6月",
        "Jul" => "7月",
        "Aug" => "8月",
        "Sep" => "9月",
        "Oct" => "10月",
        "Nov" => "11月",
        "Dec" => "12月",
        "Mon" => "周一",
        "Wed" => "周三",
        "Fri" => "周五",
        " sess" => " 会话",
        "in: " => "输入: ",
        "out: " => "输出: ",
        "cache: " => "缓存: ",
        "/day  " => "/天  ",
        "/hour " => "/小时 ",
        "/30m  " => "/30分  ",
        "/6m   " => "/6分   ",
        " Cost" => " 费用",
        " Tokens" => " Token数",
        " input " => " 输入 ",
        " output" => " 输出",
        "sessions " => "会话数 ",
        "active " => "活跃 ",
        "in " => "输入 ",
        "out " => "输出 ",
        "cache " => "缓存 ",
        "net " => "净增 ",
        "total " => "总计 ",
        "accepted " => "已接受 ",
        " Recent sessions" => " 近期会话",
        " Live rate status " => " 实时速率状态 ",
        "Use left/right to choose the OAuth source you want to monitor." => "用 ←→ 选择要监控的 OAuth 来源。",
        "Steady" => "平稳",
        "Watch" => "留意",
        "Slow down" => "放慢",
        "Critical" => "危险",
        "No hit forecast yet." => "暂无触发限制预测。",
        "soon" => "即将",
        "Date" => "日期",
        "Source" => "来源",
        "Dur" => "时长",
        "Cost" => "费用",
        "<1m" => "<1分",
        " Avg max $/session " => " 每会话平均峰值 $ ",
        " Trend " => " 趋势 ",
        " Last hit " => " 上次触发 ",
        "today" => "今天",
        "1d ago" => "1天前",
        "never" => "从未",
        " Session pacing " => " 会话节奏 ",
        "Select a source to inspect its current session." => "选择一个来源以查看其当前会话。",
        "^now" => "^现在",
        "no forecast yet" => "暂无预测",
        " Session token pace " => " 会话 Token 速率 ",
        " Daily session cost " => " 每日会话花费 ",
        " history " => " 历史 ",
        " hit day " => " 触发日 ",
        " projected live " => " 预测实时 ",
        " cache/in/out " => " 缓存/输入/输出 ",
        "no data" => "暂无数据",
        " No OAuth " => " 无 OAuth ",
        "No OAuth token saved for this source yet." => "尚未为此来源保存 OAuth Token。",
        "Saved OAuth token is expired; waiting for rediscovery." => "已保存的 OAuth Token 已过期，等待重新发现。",
        "Waiting for the first usage poll." => "等待首次用量轮询。",
        "Usage data is not available yet." => "用量数据暂不可用。",
        "live from session" => "会话实时数据",
        "no token" => "无 Token",
        "token expired" => "Token 已过期",
        "awaiting first poll" => "等待首次轮询",
        "last success {}" => "上次成功 {}",
        "last success " => "上次成功 ",
        " ·7d " => " ·7天 ",
        "· " => "· ",
        "× value " => "× 价值 ",
        "polled " => "已轮询 ",
        "attempts" => "次尝试",
        "ok" => "成功",
        "rate-limited" => "次限速",
        "reset" => "重置",
        "elapsed" => "已用",
        "Hit in " => "将在 ",
        "at current pace (around " => "按当前速率触发限额（约 ",
        ")." => "）。",
        "On track for ~" => "预计约 ",
        "% at reset." => "% 时达到重置。",
        "Current " => "当前 ",
        "Safe " => "安全 ",
        "Session " => "会话 ",
        " Recent rate-limit hits (" => " 近期速率限制触发 (",
        ")" => "）",
        "hit in " => "将在 ",
        "% at reset" => "% 时达到重置",
        "current " => "当前 ",
        "safe " => "安全 ",
        "/2m" => "/2分",
        "Projects" => "项目",
        "Display" => "显示",
        " Settings " => " 设置 ",
        " Settings — Projects " => " 设置 — 项目 ",
        " Settings — MERGE: select 2nd project " => " 设置 — 合并: 选择第二个项目 ",
        "Are you sure?" => "确定吗？",
        "Confirm" => "确认",
        "Cancel" => "取消",
        "was: " => "原名: ",
        "Expanded" => "展开",
        "Compact" => "紧凑",
        " ◀ active " => " ◀ 当前 ",
        "Star" => "加星标",
        "Unstar" => "取消星标",
        "Show" => "显示",
        "Hide" => "隐藏",
        "Back" => "返回",
        " [split]" => " [已拆分]",
        " [merged]" => " [已合并]",
        " Reset all overrides " => " 重置所有覆盖项 ",
        " Rename project " => " 重命名项目 ",
        "  Heatmap mode: " => "  热力图模式: ",
        "  Language: " => "  语言: ",
        " sessions" => " 次会话",
        "Navigate   " => "导航   ",
        "Expand   " => "展开   ",
        "Merge   " => "合并   ",
        "Split all   " => "全部拆分   ",
        "Reset   " => "重置   ",
        "Rename   " => "重命名   ",
        "Extract   " => "提取   ",
        "Search   " => "搜索   ",
        "Reset all   " => "重置全部   ",
        "Confirm merge   " => "确认合并   ",
        "Switch tab   " => "切换标签   ",
        "Toggle   " => "切换   ",
        "Parsing session files" => "正在解析会话文件",
        "Press q to quit" => "按 q 退出",
        "Cache rebuilt" => "缓存已重建",
        "Cache was unreadable" => "缓存不可读",
        "press any key to dismiss" => "按任意键关闭",
        " available" => " 可更新",
        "ⓘ Older history is backfilled: token totals only (no per-project / line / model detail)." => "ⓘ 较早历史为回填数据：仅 Token 总量（无按项目/行/模型的明细）。",
        "Tab Period   ⇧Tab Source   ←→ Project   ↑↓ Scroll   r Refresh   t Rate tracking   . Settings   q Quit" => "Tab 时段   ⇧Tab 来源   ←→ 项目   ↑↓ 滚动   r 刷新   t 速率追踪   . 设置   q 退出",
        "Esc Back   Tab Period   ←→ Project   r Refresh   t Rate tracking   q Quit" => "Esc 返回   Tab 时段   ←→ 项目   r 刷新   t 速率追踪   q 退出",
        "⟳ Reloading…" => "⟳ 重新加载中…",
        "←→ Select source   r Refresh   t Dashboard   q Quit" => "←→ 选择来源   r 刷新   t 仪表板   q 退出",
        _ => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_returns_english_under_en() {
        set_lang(Lang::En);
        assert_eq!(t(" Cost USD "), " Cost USD ");
    }

    #[test]
    fn t_translates_under_zh() {
        set_lang(Lang::Zh);
        assert_eq!(t(" Cost USD "), " 费用 USD ");
        set_lang(Lang::En); // reset for other tests
    }

    #[test]
    fn t_falls_back_to_input_for_unknown() {
        set_lang(Lang::Zh);
        assert_eq!(t("zzz-not-a-key"), "zzz-not-a-key");
        set_lang(Lang::En); // reset
    }

    #[test]
    fn detect_reads_zh_from_env() {
        assert_eq!(from_lang_str("zh_CN.UTF-8"), Lang::Zh);
        assert_eq!(from_lang_str("en_US"), Lang::En);
    }
}
