use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chrono::{Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

use crate::config::discovery;
use crate::config::overrides::{self, Overrides};
use crate::config::settings::Settings;
use crate::data::cache;
use crate::data::hit_history::{self, HitHistory};
use crate::data::index::EventIndex;
use crate::data::oauth::{OAuthCredential, UsagePoller};
use crate::data::parser;
use crate::data::rate_history::{self, RateHistory};
use crate::data::rate_limits::RateLimitHit;
use crate::data::tokens::{DailyTokens, MinuteTokens};
use crate::ui::cards;
use crate::ui::heatmap;
use crate::ui::settings_view::{KeyResult, SettingsState};
use crate::ui::time_filter::{TimeFilter, date_in_filter, filter_daily};
use crate::update_check::{self, UpdateInfo};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

pub(crate) enum View {
    RateTracking,
    Main,
    Settings(Box<SettingsState>),
}

/// A selectable source in the ⇧Tab cycle: a display name, the cache-root
/// filter it applies, and the EventIndex root filter (the index is Claude-only,
/// so a Codex view passes the codex root and gets empty index stats).
#[derive(Clone)]
pub(crate) struct SourceEntry {
    pub(crate) name: String,
    pub(crate) roots: crate::data::cache::RootFilter,
}

pub(crate) struct CachedKpi {
    pub(crate) cost: f64,
    pub(crate) streak: u32,
    pub(crate) active: usize,
    pub(crate) total_days: usize,
    pub(crate) avg: u64,
    pub(crate) efficiency: f64,
}

impl CachedKpi {
    fn compute(filtered: &DailyTokens) -> Self {
        let (active, total_days) = filtered.active_and_total_days();
        Self {
            cost: filtered.total_cost(),
            streak: filtered.current_streak(),
            active,
            total_days,
            avg: filtered.avg_tokens_per_active_day(),
            efficiency: filtered.avg_efficiency(),
        }
    }
}

type ReloadResult = (cache::Cache, EventIndex);

struct DiscoveryRefresh {
    discovery_changed: bool,
    should_poll_now: bool,
}

struct DiscoveryResult {
    raw_groups: Vec<discovery::ProjectGroup>,
    root_cwd_map: HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
    session_map: HashMap<String, (String, String)>,
    fresh_credentials: Vec<OAuthCredential>,
}

// ---------------------------------------------------------------------------
// Sub-structs for logical grouping
// ---------------------------------------------------------------------------

/// Raw parsed data (cache + compact index + derived time-series).
pub(crate) struct AppData {
    pub(crate) merged_cache: cache::Cache,
    pub(crate) index: EventIndex,
    pub(crate) daily_tokens: DailyTokens,
    pub(crate) thresholds: heatmap::Thresholds,
    pub(crate) minute_tokens: MinuteTokens,
    pub(crate) rate_limit_hits: Vec<RateLimitHit>,
    pub(crate) oauth_credentials: Vec<OAuthCredential>,
    pub(crate) rate_history: RateHistory,
    #[allow(dead_code)]
    pub(crate) hit_history: HitHistory,
}

/// Project configuration (discovery results + user overrides).
pub(crate) struct AppConfig {
    pub(crate) overrides: Overrides,
    pub(crate) settings: Settings,
    pub(crate) groups: Vec<discovery::ProjectGroup>,
    pub(crate) raw_groups: Arc<Vec<discovery::ProjectGroup>>,
    pub(crate) session_map: Arc<HashMap<String, (String, String)>>,
    pub(crate) sources: Vec<SourceEntry>,
}

/// Per-model breakdown for the Codex source view, where the cards/detail
/// machinery doesn't apply (Codex has no ProjectGroup). Built from the model
/// stats so Codex usage is shown split by specific model (gpt-5.5 /
/// gpt-5.3-codex) — both as a list and a stacked-by-model cost chart.
pub(crate) struct CodexBreakdown {
    /// (model label, tokens, cost) sorted by cost descending.
    pub(crate) rows: Vec<(String, u64, f64)>,
    /// model label → sorted daily cost series (for the stacked chart).
    pub(crate) daily: Vec<(String, Vec<(NaiveDate, f64)>)>,
}

/// Pre-computed values for the current view (rebuilt when filters change).
pub(crate) struct RenderCache {
    pub(crate) filtered: DailyTokens,
    pub(crate) kpi: CachedKpi,
    pub(crate) minute_model: HashMap<(String, String), HashMap<(NaiveDate, u16), f64>>,
    pub(crate) cards: Vec<cards::ProjectCard>,
    pub(crate) range: (NaiveDate, NaiveDate),
    /// Maps display position → index in `config.groups`.
    /// Navigation with ←/→ follows this order so it matches the card rendering order.
    pub(crate) display_order: Vec<usize>,
    /// Present only in the Codex source view; drives the Codex per-model panel.
    pub(crate) codex_breakdown: Option<CodexBreakdown>,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub(crate) struct App {
    pub(crate) data: AppData,
    pub(crate) config: AppConfig,
    pub(crate) render: RenderCache,

    pub(crate) view: View,
    pub(crate) time_filter: TimeFilter,
    pub(crate) source_index: usize,
    pub(crate) project_index: Option<usize>,

    pub(crate) render_dirty: bool,
    pub(crate) card_scroll: usize,
    pub(crate) rate_tracking_selected: usize,

    pub(crate) reloading: bool,
    reload_tx: mpsc::Sender<ReloadResult>,
    reload_rx: mpsc::Receiver<ReloadResult>,

    pub(crate) discovery_in_flight: bool,
    discovery_tx: mpsc::Sender<DiscoveryResult>,
    discovery_rx: mpsc::Receiver<DiscoveryResult>,
    manual_refresh_pending: bool,

    pub(crate) start_time: std::time::Instant,
    pub(crate) last_reload: std::time::Instant,
    last_refresh: std::time::Instant,

    usage_poller: UsagePoller,
    pub(crate) reload_interval: Duration,
    refresh_requested: bool,

    pub(crate) update_info: Option<UpdateInfo>,
    update_rx: mpsc::Receiver<UpdateInfo>,

    /// Outcome of the initial on-disk cache load. `Migrated` and
    /// `Recovered` both trigger a one-time banner but with distinct
    /// messages so the user can tell a planned schema bump apart from a
    /// corrupted cache. Dismissed on the first user keypress.
    pub(crate) cache_load_state: cache::CacheLoad,
}

impl App {
    /// Runs `App::new()` on a background thread so the main thread can
    /// render a loading screen while project discovery and JSONL parsing
    /// happen. The `App` is boxed to keep the channel slot small.
    pub(crate) fn spawn_load() -> mpsc::Receiver<Box<App>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Box::new(App::new()));
        });
        rx
    }

    pub(crate) fn new() -> Self {
        let (raw_groups, root_cwd_map, session_map) =
            discovery::discover_project_groups_with_root_map();
        let raw_groups = Arc::new(raw_groups);
        let session_map = Arc::new(session_map);
        let (merged_cache, index, mut cache_load_state) = load_data(&raw_groups, &session_map);
        // Debug escape hatch for screenshotting the banners after a
        // migration already happened. Set CCMETER_FORCE_BANNER=recovered
        // for the corruption banner, anything else for the migration one.
        if let Ok(kind) = std::env::var("CCMETER_FORCE_BANNER") {
            cache_load_state = if kind.eq_ignore_ascii_case("recovered") {
                cache::CacheLoad::Recovered
            } else {
                cache::CacheLoad::Migrated
            };
        }

        let (daily_tokens, thresholds) =
            compute_daily_and_thresholds(&merged_cache, &cache::RootFilter::All, None);
        let minute_tokens = index.build_minute_tokens(&cache::RootFilter::All, None);

        let root_paths = sorted_root_paths(&root_cwd_map);
        let mut fresh_hits = crate::data::rate_limits::discover_rate_limit_hits(&root_paths);
        compute_hit_tokens(&mut fresh_hits, &index);
        let mut hit_history = hit_history::load();
        let merge = hit_history.merge_fresh_hits(&fresh_hits);
        let rate_limit_hits = merge.hits;
        if merge.changed {
            hit_history::save(&hit_history);
        }
        let oauth_credentials = crate::data::oauth::discover_credentials_with_usage(&root_paths);
        let rate_history = rate_history::load();

        let mut overrides = Overrides::load();
        let groups = overrides::apply_overrides(&raw_groups, &mut overrides);
        let has_codex = merged_cache
            .roots()
            .any(|(r, _)| r == crate::data::codex::CODEX_ROOT);
        let sources = build_source_list(&root_cwd_map, has_codex);
        let source_index: usize = 0;
        let project_index: Option<usize> = None;
        let settings = Settings::load();
        let time_filter = settings.time_filter.unwrap_or(TimeFilter::All);
        let initial_view = match settings.last_view.as_deref() {
            Some("rate_tracking") => View::RateTracking,
            _ => View::Main, // includes "main" and any unknown value
        };

        let cwds_filter = project_cwds_static(&groups, project_index);

        let render = build_render_cache(
            &daily_tokens,
            &minute_tokens,
            &index,
            &groups,
            &merged_cache,
            &overrides,
            &sources[source_index].roots,
            cwds_filter.as_deref(),
            time_filter,
        );

        let (reload_tx, reload_rx) = mpsc::channel::<ReloadResult>();
        let (discovery_tx, discovery_rx) = mpsc::channel::<DiscoveryResult>();
        let usage_poller = UsagePoller::new(&oauth_credentials);
        let update_rx = update_check::spawn_check();

        let mut app = App {
            data: AppData {
                merged_cache,
                index,
                daily_tokens,
                thresholds,
                minute_tokens,
                rate_limit_hits,
                oauth_credentials,
                rate_history,
                hit_history,
            },
            config: AppConfig {
                overrides,
                settings,
                groups,
                raw_groups,
                session_map,
                sources,
            },
            render,
            view: initial_view,
            time_filter,
            source_index,
            project_index,
            render_dirty: false,
            card_scroll: 0,
            rate_tracking_selected: 0,
            reloading: false,
            reload_tx,
            reload_rx,
            discovery_in_flight: false,
            discovery_tx,
            discovery_rx,
            manual_refresh_pending: false,
            start_time: std::time::Instant::now(),
            last_reload: std::time::Instant::now(),
            last_refresh: std::time::Instant::now(),
            usage_poller,
            reload_interval: Duration::from_secs(5 * 60),
            refresh_requested: false,
            update_info: None,
            update_rx,
            cache_load_state,
        };
        app.record_rate_history();
        app
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn project_cwds(&self) -> Option<Vec<String>> {
        let group_index = self.project_index.map(|i| self.render.display_order[i]);
        project_cwds_static(&self.config.groups, group_index)
    }

    /// True when the cache holds synthetic backfill roots (history older than
    /// the live JSONL window). Used to render a low-fidelity notice.
    pub(crate) fn has_backfilled_history(&self) -> bool {
        self.data
            .merged_cache
            .roots()
            .any(|(root, _)| root.starts_with("backfill:"))
    }

    fn recompute_tokens(&mut self) {
        let cwds_filter = self.project_cwds();
        let entry = self.config.sources[self.source_index].clone();
        let (d, t) = compute_daily_and_thresholds(
            &self.data.merged_cache,
            &entry.roots,
            cwds_filter.as_deref(),
        );
        self.data.daily_tokens = d;
        self.data.thresholds = t;
        self.data.minute_tokens = self
            .data
            .index
            .build_minute_tokens(&entry.roots, cwds_filter.as_deref());
        self.render_dirty = true;
    }

    fn recompute_render_cache(&mut self) {
        let cwds_filter = self.project_cwds();
        let entry = self.config.sources[self.source_index].clone();
        let prev_display_order = std::mem::take(&mut self.render.display_order);
        self.render = build_render_cache(
            &self.data.daily_tokens,
            &self.data.minute_tokens,
            &self.data.index,
            &self.config.groups,
            &self.data.merged_cache,
            &self.config.overrides,
            &entry.roots,
            cwds_filter.as_deref(),
            self.time_filter,
        );
        // When viewing a single project, build_render_cache produces a
        // display_order with only one entry. Preserve the full ordering
        // so that ←/→ navigation keeps working.
        if self.project_index.is_some() {
            self.render.display_order = prev_display_order;
        }
        self.render_dirty = false;
    }

    fn refresh_rate_limit_hits(&mut self) {
        let root_paths: Vec<std::path::PathBuf> =
            session_map_root_inventory(&self.config.session_map)
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect();
        let mut fresh_hits = crate::data::rate_limits::discover_rate_limit_hits(&root_paths);
        compute_hit_tokens(&mut fresh_hits, &self.data.index);
        let merge = self.data.hit_history.merge_fresh_hits(&fresh_hits);
        self.data.rate_limit_hits = merge.hits;
        if merge.changed {
            hit_history::save(&self.data.hit_history);
        }
    }

    fn apply_discovery_result(&mut self, result: DiscoveryResult) -> DiscoveryRefresh {
        let selected_source_name = self
            .config
            .sources
            .get(self.source_index)
            .map(|s| s.name.clone());
        let selected_project_root = self
            .project_index
            .and_then(|i| self.render.display_order.get(i))
            .map(|&gi| self.config.groups[gi].root_key());
        let selected_tracking_root = self
            .data
            .oauth_credentials
            .get(self.rate_tracking_selected)
            .map(|cred| cred.source_root.to_string_lossy().to_string());
        let old_roots = session_map_root_inventory(&self.config.session_map);

        let DiscoveryResult {
            raw_groups,
            root_cwd_map,
            session_map,
            mut fresh_credentials,
        } = result;

        let discovery_changed = old_roots != root_map_inventory(&root_cwd_map)
            || self.config.session_map.as_ref() != &session_map;

        preserve_credential_state(&self.data.oauth_credentials, &mut fresh_credentials);
        let should_poll_now = self.usage_poller.refresh_entries(&fresh_credentials);

        let raw_groups = Arc::new(raw_groups);
        let session_map = Arc::new(session_map);
        let groups = overrides::apply_overrides(&raw_groups, &mut self.config.overrides);
        let has_codex = self
            .data
            .merged_cache
            .roots()
            .any(|(r, _)| r == crate::data::codex::CODEX_ROOT);
        let sources = build_source_list(&root_cwd_map, has_codex);

        self.config.raw_groups = raw_groups;
        self.config.session_map = session_map;
        self.config.groups = groups;
        self.config.sources = sources;
        self.data.oauth_credentials = fresh_credentials;

        self.source_index = selected_source_name
            .as_deref()
            .and_then(|name| {
                self.config
                    .sources
                    .iter()
                    .position(|candidate| candidate.name == name)
            })
            .unwrap_or(0)
            .min(self.config.sources.len().saturating_sub(1));

        self.rate_tracking_selected = selected_tracking_root
            .as_deref()
            .and_then(|root| {
                self.data
                    .oauth_credentials
                    .iter()
                    .position(|cred| cred.source_root.to_string_lossy().as_ref() == root)
            })
            .unwrap_or(0)
            .min(self.data.oauth_credentials.len().saturating_sub(1));

        self.project_index = None;
        self.recompute_tokens();
        self.recompute_render_cache();

        self.project_index = selected_project_root.as_deref().and_then(|root| {
            self.render
                .display_order
                .iter()
                .position(|&gi| self.config.groups[gi].root_key() == root)
        });
        if self.project_index.is_some() {
            self.recompute_tokens();
            self.recompute_render_cache();
        }

        DiscoveryRefresh {
            discovery_changed,
            should_poll_now,
        }
    }

    /// Persist estimated token capacity for active 5h sessions into rate-history.
    fn record_rate_history(&mut self) {
        let mut changed = false;

        for cred in &self.data.oauth_credentials {
            let source_root = cred.source_root.to_string_lossy().to_string();
            let Some(usage) = &cred.usage else {
                continue;
            };
            let Some(five_h) = &usage.five_hour else {
                continue;
            };
            let Some(resets_at_str) = &five_h.resets_at else {
                continue;
            };
            let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str) else {
                continue;
            };

            let utilization = five_h.utilization;
            if utilization <= 0.0 {
                continue;
            }

            let resets_utc = resets_at.with_timezone(&chrono::Utc);
            let session_start_utc = resets_utc - chrono::Duration::hours(5);
            let now_utc = chrono::Utc::now();

            // Skip recording when the session is too young (< 30 min):
            // the estimate is unstable because local tokens and API
            // utilization don't update at the same rate.
            let elapsed_min = (now_utc - session_start_utc).num_seconds().max(0) as f64 / 60.0;
            if elapsed_min < 30.0 || utilization < 2.0 {
                continue;
            }

            let start_local = session_start_utc
                .with_timezone(&chrono::Local)
                .naive_local();
            let end_local = now_utc.with_timezone(&chrono::Local).naive_local();
            let tokens = self
                .data
                .index
                .tokens_in_window(&source_root, start_local, end_local);
            if tokens == 0 {
                continue;
            }

            let estimated_tokens = (tokens as f64 / (utilization / 100.0)).round() as u64;
            let per_model =
                self.data
                    .index
                    .per_model_breakdown_in_window(&source_root, start_local, end_local);
            let session_date = start_local.date();
            self.data.rate_history.record(
                &source_root,
                resets_at_str,
                estimated_tokens,
                per_model,
                session_date,
            );
            changed = true;
        }

        if changed {
            rate_history::save(&self.data.rate_history);
        }
    }

    // ------------------------------------------------------------------
    // Event loop helpers (called from main)
    // ------------------------------------------------------------------

    pub(crate) fn pre_render(&mut self) {
        if let View::Settings(state) = &mut self.view {
            state.tick = (self.start_time.elapsed().as_millis() / 80) as usize;
        }
        if self.update_info.is_none()
            && let Ok(info) = self.update_rx.try_recv()
        {
            self.update_info = Some(info);
        }
        if self.render_dirty {
            self.recompute_render_cache();
        }
    }

    fn switch_view(&mut self, view: View) {
        let key = match &view {
            View::RateTracking => "rate_tracking",
            View::Main => "main",
            View::Settings(_) => return,
        };
        self.view = view;
        self.render_dirty = true;
        self.config.settings.last_view = Some(key.to_string());
        self.config.settings.save();
    }

    pub(crate) fn handle_reload(&mut self) {
        let manual_refresh_requested = self.refresh_requested;
        let refresh_due =
            manual_refresh_requested || self.last_refresh.elapsed() >= self.reload_interval;

        if refresh_due && !self.discovery_in_flight && !self.reloading {
            spawn_discovery(&self.discovery_tx);
            self.discovery_in_flight = true;
            if manual_refresh_requested {
                self.manual_refresh_pending = true;
            }
            self.refresh_requested = false;
            self.last_refresh = std::time::Instant::now();
        }

        let mut discovery_changed = false;
        let mut manual_refresh_completed = false;
        if self.discovery_in_flight
            && let Ok(result) = self.discovery_rx.try_recv()
        {
            let refresh = self.apply_discovery_result(result);
            discovery_changed = refresh.discovery_changed;
            if refresh.should_poll_now || self.manual_refresh_pending {
                self.usage_poller.force_poll_all();
            }
            manual_refresh_completed = self.manual_refresh_pending;
            self.manual_refresh_pending = false;
            self.discovery_in_flight = false;
        }

        let usage_updated = self.usage_poller.poll(&mut self.data.oauth_credentials);

        if (manual_refresh_completed || discovery_changed || usage_updated)
            && !self.reloading
            && self.last_reload.elapsed() >= Duration::from_millis(500)
        {
            spawn_reload(
                &self.config.raw_groups,
                &self.config.session_map,
                &self.reload_tx,
            );
            self.reloading = true;
            self.last_reload = std::time::Instant::now();
        }

        if self.reloading
            && let Ok((c, idx)) = self.reload_rx.try_recv()
        {
            self.data.merged_cache = c;
            self.data.index = idx;
            self.reloading = false;
            self.recompute_tokens();
            self.refresh_rate_limit_hits();
            self.record_rate_history();
        }
    }

    /// Returns true when any step of a refresh chain is in progress
    /// (pending request, discovery thread, or JSONL reload thread).
    pub(crate) fn is_busy(&self) -> bool {
        self.reloading
            || self.discovery_in_flight
            || self.refresh_requested
            || self.usage_poller.has_in_flight()
    }

    /// Returns false if the app should quit.
    pub(crate) fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return true;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        // Dismiss the cache-state notice on the first user keypress.
        if self.cache_load_state != cache::CacheLoad::Fresh {
            self.cache_load_state = cache::CacheLoad::Fresh;
            self.render_dirty = true;
        }

        // Tab/BackTab cycle time filters globally, except in Settings/RateTracking.
        if !matches!(self.view, View::Settings(_) | View::RateTracking) {
            match key.code {
                KeyCode::Tab => {
                    self.time_filter = self.time_filter.next();
                    self.config.settings.time_filter = Some(self.time_filter);
                    self.config.settings.save();
                    self.card_scroll = 0;
                    self.render_dirty = true;
                    return true;
                }
                KeyCode::BackTab => {
                    self.source_index = (self.source_index + 1) % self.config.sources.len();
                    self.card_scroll = 0;
                    self.recompute_tokens();
                    return true;
                }
                _ => {}
            }
        }

        match &mut self.view {
            View::RateTracking => match key.code {
                KeyCode::Char('`') | KeyCode::Char('t') => {
                    self.switch_view(View::Main);
                }
                KeyCode::Char('q') => return false,
                KeyCode::Right | KeyCode::Char('l') => {
                    let n = self.data.oauth_credentials.len();
                    if n > 0 {
                        self.rate_tracking_selected = (self.rate_tracking_selected + 1) % n;
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    let n = self.data.oauth_credentials.len();
                    if n > 0 {
                        self.rate_tracking_selected = (self.rate_tracking_selected + n - 1) % n;
                    }
                }
                KeyCode::Char('r') if !self.is_busy() => {
                    self.refresh_requested = true;
                }
                _ => {}
            },
            View::Main => match key.code {
                KeyCode::Esc if self.project_index.is_some() => {
                    self.project_index = None;
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                KeyCode::Char('q') => return false,
                KeyCode::Char('`') | KeyCode::Char('t') => {
                    self.switch_view(View::RateTracking);
                }
                KeyCode::Char('r') if !self.is_busy() => {
                    self.refresh_requested = true;
                }
                KeyCode::Char('.') => {
                    self.view = View::Settings(Box::new(SettingsState::new(&self.config.groups)));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.card_scroll = self.card_scroll.saturating_add(1);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.card_scroll = self.card_scroll.saturating_sub(1);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    let len = self.render.display_order.len();
                    self.project_index = match self.project_index {
                        None if len > 0 => Some(0),
                        Some(i) if i + 1 < len => Some(i + 1),
                        _ => None,
                    };
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    let len = self.render.display_order.len();
                    self.project_index = match self.project_index {
                        None if len > 0 => Some(len - 1),
                        Some(0) => None,
                        Some(i) => Some(i - 1),
                        _ => None,
                    };
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                _ => {}
            },
            View::Settings(state) => {
                match state.handle_key(
                    key,
                    &self.config.groups,
                    &mut self.config.overrides,
                    &mut self.config.settings,
                ) {
                    KeyResult::Rebuild => {
                        let selected = state.selected;
                        let tick = state.tick;
                        let tab = state.active_tab();
                        self.config.groups = overrides::apply_overrides(
                            &self.config.raw_groups,
                            &mut self.config.overrides,
                        );
                        if let Some(idx) = self.project_index
                            && idx >= self.render.display_order.len()
                        {
                            self.project_index = None;
                        }
                        let mut new_state =
                            SettingsState::with_selected(&self.config.groups, selected, Some(tab));
                        new_state.tick = tick;
                        self.view = View::Settings(Box::new(new_state));
                        self.render_dirty = true;
                    }
                    KeyResult::Close => {
                        self.view = View::Main;
                        self.render_dirty = true;
                    }
                    KeyResult::Continue => {}
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

fn project_cwds_static(
    groups: &[discovery::ProjectGroup],
    idx: Option<usize>,
) -> Option<Vec<String>> {
    idx.map(|i| {
        groups[i]
            .sources
            .iter()
            .filter_map(|s| s.cwd.clone())
            .collect()
    })
}

fn preserve_credential_state(existing: &[OAuthCredential], fresh: &mut [OAuthCredential]) {
    let existing_by_root: HashMap<String, &OAuthCredential> = existing
        .iter()
        .map(|cred| (cred.source_root.to_string_lossy().to_string(), cred))
        .collect();

    for cred in fresh {
        let root = cred.source_root.to_string_lossy().to_string();
        let Some(previous) = existing_by_root.get(&root) else {
            continue;
        };

        cred.stats = previous.stats.clone();
        // Only carry over the cached usage when the credential is still
        // pollable: if the token has expired we stop fetching (see
        // `UsagePoller::refresh_entries`), so reusing a stale snapshot would
        // keep rendering a "live" session for an invalid credential.
        if previous.access_token == cred.access_token && !cred.is_expired() {
            cred.usage = previous.usage.clone();
        }
    }
}

fn sorted_root_paths(
    root_map: &HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
) -> Vec<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = root_map.keys().cloned().collect();
    roots.sort();
    roots
}

fn root_map_inventory(
    root_map: &HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
) -> Vec<String> {
    sorted_root_paths(root_map)
        .into_iter()
        .map(|root| root.to_string_lossy().to_string())
        .collect()
}

fn session_map_root_inventory(session_map: &HashMap<String, (String, String)>) -> Vec<String> {
    let mut roots: Vec<String> = session_map.values().map(|(root, _)| root.clone()).collect();
    roots.sort();
    roots.dedup();
    roots
}

#[allow(clippy::too_many_arguments)]
fn build_render_cache(
    daily_tokens: &DailyTokens,
    minute_tokens: &MinuteTokens,
    index: &EventIndex,
    groups: &[discovery::ProjectGroup],
    merged_cache: &cache::Cache,
    overrides: &Overrides,
    roots: &cache::RootFilter,
    project_cwds: Option<&[String]>,
    time_filter: TimeFilter,
) -> RenderCache {
    let today_snap = Local::now().date_naive();
    let subday = time_filter.subday_start();

    // For sub-day filters (1H, 12H), build DailyTokens from minute-level data
    // so that KPI values reflect the actual time window.
    let filtered = if let Some((sd, sm)) = subday {
        minute_tokens.to_daily_filtered(sd, sm, today_snap)
    } else {
        filter_daily(daily_tokens, time_filter)
    };

    let kpi = CachedKpi::compute(&filtered);
    let cwd_to_root = cards::build_cwd_to_root(groups);
    let date_filter = |d: NaiveDate| date_in_filter(d, time_filter, today_snap);
    let stats = index.build_model_stats(
        &cwd_to_root,
        roots,
        &date_filter,
        project_cwds,
        time_filter.is_intraday(),
        subday,
    );

    // For sub-day filters, build a cache from the index filtered by minute
    // so that card costs reflect the actual time window.
    let effective_cache;
    let cache_ref = if let Some((sd, sm)) = subday {
        effective_cache = index.build_subday_cache(sd, sm, today_snap);
        &effective_cache
    } else {
        merged_cache
    };
    let cards = cards::build_cards(
        groups,
        cache_ref,
        overrides,
        roots,
        date_filter,
        &stats.tokens,
        project_cwds,
        &stats.daily_costs,
    );

    // Build a mapping from display position (card order) → group index.
    let root_to_group: std::collections::HashMap<String, usize> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.root_key(), i))
        .collect();
    let display_order: Vec<usize> = cards
        .iter()
        .filter_map(|c| root_to_group.get(&c.root_key).copied())
        .collect();

    let range = compute_range(&filtered, time_filter, today_snap);

    // In the Codex source view there are no cards (Codex has no ProjectGroup),
    // so surface a dedicated per-model breakdown built from the model stats.
    // Built whenever Codex is part of the view (All or the dedicated Codex
    // source) — Codex has no ProjectGroup/card, so this panel is how its
    // per-model usage surfaces. Empty rows ⇒ no panel rendered.
    let codex_breakdown = roots
        .matches(crate::data::codex::CODEX_ROOT)
        .then(|| build_codex_breakdown(&stats))
        .filter(|b| !b.rows.is_empty());

    RenderCache {
        filtered,
        kpi,
        minute_model: stats.minute_costs,
        cards,
        range,
        display_order,
        codex_breakdown,
    }
}

/// Collect the per-model breakdown for the Codex root from model stats:
/// (label, tokens, total cost) rows + per-model daily cost series, both
/// ordered by cost descending so the list, legend and stacked chart agree.
fn build_codex_breakdown(stats: &crate::data::index::ModelStats) -> CodexBreakdown {
    use crate::data::codex::CODEX_ROOT;

    let mut labels: Vec<String> = stats
        .tokens
        .keys()
        .filter(|(rk, _)| rk == CODEX_ROOT)
        .map(|(_, label)| label.clone())
        .collect();
    labels.sort();
    labels.dedup();

    let mut rows: Vec<(String, u64, f64)> = Vec::new();
    let mut daily: Vec<(String, Vec<(NaiveDate, f64)>)> = Vec::new();
    for label in labels {
        let key = (CODEX_ROOT.to_string(), label.clone());
        let tokens = *stats.tokens.get(&key).unwrap_or(&0);
        let day_map = stats.daily_costs.get(&key);
        let cost: f64 = day_map.map(|m| m.values().sum()).unwrap_or(0.0);
        let mut series: Vec<(NaiveDate, f64)> = day_map
            .map(|m| m.iter().map(|(&d, &c)| (d, c)).collect())
            .unwrap_or_default();
        series.sort_by_key(|&(d, _)| d);
        rows.push((label.clone(), tokens, cost));
        if !series.is_empty() {
            daily.push((label, series));
        }
    }

    let by_cost_desc = |a: f64, b: f64| b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal);
    rows.sort_by(|a, b| by_cost_desc(a.2, b.2));
    let cost_of = |label: &str| rows.iter().find(|r| r.0 == label).map(|r| r.2).unwrap_or(0.0);
    daily.sort_by(|a, b| by_cost_desc(cost_of(&a.0), cost_of(&b.0)));

    CodexBreakdown { rows, daily }
}

fn compute_hit_tokens(hits: &mut [RateLimitHit], index: &EventIndex) {
    for hit in hits.iter_mut() {
        let Some(dur_min) = hit.session_duration_min else {
            continue;
        };
        let dur_secs = (dur_min * 60.0) as i64;
        let session_start_utc = hit.timestamp - chrono::Duration::seconds(dur_secs);
        let start_local = session_start_utc
            .with_timezone(&chrono::Local)
            .naive_local();
        let end_local = hit.timestamp.with_timezone(&chrono::Local).naive_local();
        hit.tokens = index.tokens_in_window(&hit.source_root, start_local, end_local);
        hit.per_model =
            index.per_model_breakdown_in_window(&hit.source_root, start_local, end_local);
    }
}

fn load_data(
    raw_groups: &[discovery::ProjectGroup],
    session_map: &HashMap<String, (String, String)>,
) -> (cache::Cache, EventIndex, cache::CacheLoad) {
    let all_session_files: Vec<std::path::PathBuf> = raw_groups
        .iter()
        .flat_map(|g| g.sources.iter())
        .flat_map(|s| s.session_files.iter().cloned())
        .collect();
    let events = parser::parse_session_files(&all_session_files);

    let outcome = cache::load();
    let fresh_cache = cache::from_events(&events, session_map);
    let merged = cache::merge(outcome.cache, &fresh_cache);
    // Fold live Codex usage (~/.codex/sessions) under the `codex` root so it
    // aggregates with Claude in the "All" view and persists. The codex root
    // never collides with Claude/backfill roots, so totals in "All" are
    // additive, not double counted. This runs off the UI thread (load_data is
    // always called from a background loader/reload thread), so the extra
    // parse is not user-facing.
    //
    // Codex sessions are immutable historical files, so a re-parse is always
    // authoritative: drop any persisted codex root first so the fresh parse
    // REPLACES it rather than being high-water-marked against it. Otherwise
    // stale values (e.g. the pre-fresh-input cache-inclusive counts) would win
    // the max-merge and the corrected, lower counts would never take effect.
    let codex_deltas = crate::data::codex::collect_codex_deltas();
    let (codex_cache, _codex_cwds) = crate::data::codex::aggregate(&codex_deltas);
    let mut merged = merged;
    merged.remove_root(crate::data::codex::CODEX_ROOT);
    let merged = cache::merge(merged, &codex_cache);
    cache::save(&merged);

    // Also fold the same Codex deltas into the event index so Codex usage
    // appears in per-model breakdowns (rate tracking + dashboard), split by
    // specific model (gpt-5.5 / gpt-5.3-codex).
    let mut index = EventIndex::build(&events, session_map);
    index.fold_codex(&codex_deltas);
    (merged, index, outcome.state)
}

fn spawn_reload(
    raw_groups: &Arc<Vec<discovery::ProjectGroup>>,
    session_map: &Arc<HashMap<String, (String, String)>>,
    tx: &mpsc::Sender<ReloadResult>,
) {
    let raw_groups = Arc::clone(raw_groups);
    let session_map = Arc::clone(session_map);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let (cache, index, _) = load_data(&raw_groups, &session_map);
        let _ = tx.send((cache, index));
    });
}

fn spawn_discovery(tx: &mpsc::Sender<DiscoveryResult>) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let (raw_groups, root_cwd_map, session_map) =
            discovery::discover_project_groups_with_root_map();
        let root_paths = sorted_root_paths(&root_cwd_map);
        let fresh_credentials = crate::data::oauth::discover_credentials(&root_paths);
        let _ = tx.send(DiscoveryResult {
            raw_groups,
            root_cwd_map,
            session_map,
            fresh_credentials,
        });
    });
}

fn compute_daily_and_thresholds(
    cache: &cache::Cache,
    roots: &cache::RootFilter,
    project_cwds: Option<&[String]>,
) -> (DailyTokens, heatmap::Thresholds) {
    let daily = cache::to_daily_tokens_filtered(cache, roots, project_cwds);
    let t = heatmap::Thresholds {
        input: heatmap::compute_thresholds(&daily.input),
        output: heatmap::compute_thresholds(&daily.output),
        lines_changed: heatmap::compute_thresholds(&daily.lines_accepted),
    };
    (daily, t)
}

fn compute_range(
    filtered: &DailyTokens,
    tf: TimeFilter,
    today: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    match tf {
        TimeFilter::Hour1 | TimeFilter::Hour12 | TimeFilter::Today => (today, today),
        TimeFilter::LastWeek => (today - chrono::Duration::days(6), today),
        TimeFilter::LastMonth => (today - chrono::Duration::days(29), today),
        TimeFilter::All => {
            let earliest = filtered
                .cost
                .keys()
                .chain(filtered.input.keys())
                .min()
                .copied()
                .unwrap_or(today);
            (earliest, today)
        }
    }
}

// root_map holds only Claude install roots; the codex root is injected into merged_cache by load_data and is never in root_map, so the has_codex branch is checked first.
fn build_source_list(
    root_map: &HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
    has_codex: bool,
) -> Vec<SourceEntry> {
    use crate::data::cache::RootFilter;
    use crate::data::codex::CODEX_ROOT;

    // Provider view: when Codex usage is present, expose exactly three
    // provider-level sources instead of per-install-dir Claude roots.
    if has_codex {
        return vec![
            SourceEntry {
                name: "All".to_string(),
                roots: RootFilter::All,
            },
            SourceEntry {
                name: "Claude Code".to_string(),
                roots: RootFilter::Exclude(CODEX_ROOT.to_string()),
            },
            SourceEntry {
                name: "Codex".to_string(),
                roots: RootFilter::Only(CODEX_ROOT.to_string()),
            },
        ];
    }

    let home = dirs::home_dir().unwrap_or_default();

    if root_map.len() <= 1 {
        return vec![SourceEntry {
            name: "All".to_string(),
            roots: RootFilter::All,
        }];
    }

    let mut roots: Vec<&std::path::PathBuf> = root_map.keys().collect();
    roots.sort();

    let mut sources = vec![SourceEntry {
        name: "All".to_string(),
        roots: RootFilter::All,
    }];

    for root in roots {
        let display: String = root
            .strip_prefix(&home)
            .unwrap_or(root)
            .to_string_lossy()
            .trim_start_matches('/')
            .trim_end_matches("/projects")
            .to_string();
        let root_str = root.to_string_lossy().to_string();
        sources.push(SourceEntry {
            name: display,
            roots: RootFilter::Only(root_str),
        });
    }

    sources
}
