use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::{America::Vancouver, Asia::Shanghai};
use regex::Regex;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::error::{AppError, AppResult};
use crate::models::{PredictionResult, PredictionSnapshot};

#[cfg(test)]
pub fn is_prediction_request(text: &str) -> bool {
    crate::ai::is_mentioned(text)
        && crate::ai::message_without_mention(text)
            .trim_start_matches(|character: char| {
                character.is_whitespace() || matches!(character, ':' | '：' | ',' | '，')
            })
            .starts_with("预测")
}

pub fn analyze(snapshot: &PredictionSnapshot) -> PredictionResult {
    let mut history = snapshot.history.clone();
    history.truncate(30);
    let mut counts: HashMap<i64, i64> = HashMap::new();
    let mut omissions = HashMap::new();
    for row in &history {
        for value in row.iter().filter(|value| **value >= 0 && **value <= 99) {
            *counts.entry(*value).or_default() += 1;
        }
    }
    for value in 0..=27 {
        omissions.insert(
            value,
            history
                .iter()
                .position(|row| row.contains(&value))
                .unwrap_or(history.len()) as i64,
        );
    }
    let mut values = (0..=27).collect::<Vec<i64>>();
    values.sort_by(|left, right| {
        let left_score = counts.get(left).copied().unwrap_or(0) * 2
            + omissions.get(left).copied().unwrap_or(0).min(10);
        let right_score = counts.get(right).copied().unwrap_or(0) * 2
            + omissions.get(right).copied().unwrap_or(0).min(10);
        right_score.cmp(&left_score).then(left.cmp(right))
    });
    let candidates = if history.is_empty() {
        Vec::new()
    } else {
        values.into_iter().take(3).collect()
    };
    let confidence = match history.len() {
        0..=4 => 0.2,
        5..=9 => 0.35,
        10..=24 => 0.55,
        _ => 0.7,
    };
    let trend = if history.is_empty() {
        "样本不足，暂不判断冷热".into()
    } else {
        format!("根据近 {} 期频率和遗漏计算候选方向", history.len())
    };
    PredictionResult {
        game: snapshot.game.clone(),
        period: snapshot.period.clone(),
        latest_result: snapshot.result.clone(),
        trend,
        candidates,
        confidence,
        updated_at: snapshot.updated_at,
        freshness: snapshot.freshness.clone(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: &'static str,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
}

pub const GAMES: &[Game] = &[
    Game {
        id: "pcdd",
        name: "PC28",
        aliases: &["pc蛋蛋", "PC蛋蛋", "pcdd"],
    },
    Game {
        id: "jnd",
        name: "加拿大28",
        aliases: &["加拿大", "jnd28"],
    },
    Game {
        id: "btc28",
        name: "比特币28",
        aliases: &["比特币", "BTC28"],
    },
    Game {
        id: "bj28",
        name: "北京28",
        aliases: &["北京"],
    },
    Game {
        id: "tx28",
        name: "腾讯分分彩28",
        aliases: &["腾讯分分彩", "腾讯28"],
    },
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PredictionFreshness {
    Fresh,
    Stale,
    Missing,
}

impl PredictionFreshness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PredictionSourceResult {
    pub status: PredictionFreshness,
    pub snapshot: Option<PredictionSnapshot>,
}

impl PredictionSourceResult {
    pub fn missing() -> Self {
        Self {
            status: PredictionFreshness::Missing,
            snapshot: None,
        }
    }

    pub fn from_snapshot(
        mut snapshot: PredictionSnapshot,
        now: DateTime<Utc>,
        stale_after: Duration,
    ) -> Self {
        let status = freshness_status(snapshot.updated_at, now, stale_after);
        snapshot.freshness = status.as_str().into();
        Self {
            status,
            snapshot: Some(snapshot),
        }
    }
}

pub fn freshness_status(
    updated_at: DateTime<Utc>,
    now: DateTime<Utc>,
    stale_after: Duration,
) -> PredictionFreshness {
    let stale_after = chrono::Duration::from_std(stale_after).unwrap_or(chrono::Duration::MAX);
    if now.signed_duration_since(updated_at) <= stale_after {
        PredictionFreshness::Fresh
    } else {
        PredictionFreshness::Stale
    }
}

#[async_trait]
pub trait PredictionSource: Send + Sync {
    async fn list_games(&self) -> AppResult<Vec<Game>> {
        Ok(GAMES.to_vec())
    }

    async fn fetch_live(&self, game: &Game) -> AppResult<PredictionSourceResult>;

    async fn fetch_history(&self, game: &Game, limit: usize) -> AppResult<Vec<Vec<i64>>> {
        Ok(self
            .fetch_live(game)
            .await?
            .snapshot
            .map(|snapshot| snapshot.history.into_iter().take(limit).collect())
            .unwrap_or_default())
    }

    async fn load(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        let mut result = self.fetch_live(game).await?;
        if let Some(snapshot) = result.snapshot.as_mut() {
            if snapshot.history.is_empty() {
                snapshot.history = self.fetch_history(game, 30).await?;
            }
        }
        Ok(result)
    }
}

const BCLC_KENO_CURRENT_YEAR_URL: &str =
    "https://auth.playnow.com/resources/documents/downloadable-numbers/KenoCurrentYear.zip";
const PUBLIC_KENO_FALLBACK_URL: &str = "https://pc28.help/api/keno.json";
const CWL_KL8_URL: &str = "https://www.cwl.gov.cn/cwl_admin/front/cwlkj/search/kjxx/findDrawNotice";
const DYNAMIC_CONFIG_URL: &str =
    "https://hao123wc.obs.ap-southeast-1.myhuaweicloud.com/zcs/duoduo_2.txt";
const FALLBACK_BASES: &[&str] = &[
    "https://api1.gsdatas.com",
    "https://api2.gsdatas.com",
    "https://api1.mbf52.com",
    "https://api2.mbf52.com",
];
/// 公开开奖源响应体上限。这些都是第三方/公网地址，必须在读取过程中就设上限。
const PUBLIC_RESPONSE_LIMIT: usize = crate::http_body::DEFAULT_RESPONSE_LIMIT;

/// 边读边限长地取回响应体，并把失败原因映射成本模块的错误码。
///
/// 读取机制在 [`crate::http_body`]；这里只负责错误措辞，保持本模块对外的
/// 错误码与文案不变。
async fn read_capped_body(
    response: reqwest::Response,
    limit: usize,
    source_label: &str,
) -> AppResult<Vec<u8>> {
    use crate::http_body::CappedReadError;
    crate::http_body::read_capped_body(response, limit)
        .await
        .map_err(|error| {
            let reason = match error {
                CappedReadError::Transport => "读取失败",
                CappedReadError::TooLarge => "超过大小限制",
            };
            AppError::new(
                "prediction_public_response",
                format!("{source_label}{reason}"),
            )
        })
}

#[derive(Clone)]
struct CachedPrediction {
    stored_at: Instant,
    result: PredictionSourceResult,
}

type SourceBaseCache = Option<(Instant, Vec<String>)>;

/// Public-first source. The legacy ZCG endpoint is retained only as an explicit
/// compatibility fallback when a caller supplies DH_PREDICTION_TOKEN.
pub struct PublicLotterySource {
    client: reqwest::Client,
    request_timeout: Duration,
    stale_after: Duration,
    token: String,
    live_cache: Arc<tokio::sync::Mutex<HashMap<String, CachedPrediction>>>,
    history_cache: Arc<tokio::sync::Mutex<HashMap<String, CachedPrediction>>>,
    base_cache: Arc<tokio::sync::Mutex<SourceBaseCache>>,
    request_gate: Arc<tokio::sync::Semaphore>,
    request_locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    #[cfg(test)]
    test_bases: Vec<String>,
}

impl PublicLotterySource {
    pub fn new(timeout: Duration) -> AppResult<Self> {
        let timeout = if timeout.is_zero() {
            Duration::from_secs(8)
        } else {
            timeout
        };
        let client = reqwest::Client::builder()
            // BCLC's archive endpoint intermittently resets HTTP/2 streams while
            // serving the ZIP; HTTP/1.1 is stable for both public providers.
            .http1_only()
            .connect_timeout(timeout.min(Duration::from_secs(5)))
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|_| AppError::new("prediction_client", "开奖数据客户端初始化失败"))?;
        Ok(Self {
            client,
            request_timeout: timeout,
            stale_after: Duration::from_secs(15 * 60),
            token: std::env::var("DH_PREDICTION_TOKEN").unwrap_or_default(),
            live_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            history_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            base_cache: Arc::new(tokio::sync::Mutex::new(None)),
            request_gate: Arc::new(tokio::sync::Semaphore::new(2)),
            request_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            #[cfg(test)]
            test_bases: Vec::new(),
        })
    }

    #[cfg(test)]
    fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = token.into();
        self
    }

    #[cfg(test)]
    fn with_test_base(mut self, base: impl Into<String>) -> Self {
        self.test_bases = vec![base.into()];
        self
    }

    async fn bases(&self) -> Vec<String> {
        #[cfg(test)]
        if !self.test_bases.is_empty() {
            return self.test_bases.clone();
        }
        if let Some(bases) = self
            .base_cache
            .lock()
            .await
            .as_ref()
            .filter(|(stored_at, _)| stored_at.elapsed() < Duration::from_secs(10 * 60))
            .map(|(_, bases)| bases.clone())
        {
            return bases;
        }
        let mut bases = Vec::new();
        if let Ok(response) = self.client.get(DYNAMIC_CONFIG_URL).send().await {
            // 动态配置也是公网地址，必须边读边限长。直接 `.json()` 会先把整个响应体收进内存，
            // 等于没有上限；读取失败或超限时退回空 body，让下面的解析失败并走 FALLBACK_BASES。
            let body = read_capped_body(response, PUBLIC_RESPONSE_LIMIT, "开奖源动态配置")
                .await
                .unwrap_or_default();
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) {
                for key in ["api1", "api2"] {
                    if let Some(value) = value.get(key).and_then(serde_json::Value::as_str) {
                        let normalized = value.replace("http://", "https://");
                        if normalized.starts_with("https://") {
                            bases.push(normalized.trim_end_matches('/').to_string());
                        }
                    }
                }
            }
        }
        if let Ok(value) = std::env::var("DH_PREDICTION_BASE_URL") {
            if value.starts_with("https://") {
                bases.insert(0, value.trim_end_matches('/').to_string());
            }
        }
        bases.extend(FALLBACK_BASES.iter().map(|value| (*value).to_string()));
        let mut seen = HashSet::new();
        bases.retain(|value| seen.insert(value.clone()));
        *self.base_cache.lock().await = Some((Instant::now(), bases.clone()));
        bases
    }

    async fn request_lock_for(&self, game_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.request_locks
            .lock()
            .await
            .entry(game_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    async fn fetch_network(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        match game.id {
            "jnd" => match self.fetch_bclc(game).await {
                Ok(result) => Ok(result),
                Err(_) => self.fetch_public_keno(game).await,
            },
            "pcdd" | "bj28" => self.fetch_cwl(game).await,
            "btc28" if self.token.trim().is_empty() => Err(AppError::new(
                "prediction_algorithm_unverified",
                "公开区块数据存在，但比特币28没有统一可核验的官方派生算法",
            )),
            "tx28" if self.token.trim().is_empty() => Err(AppError::new(
                "prediction_no_official_source",
                "未发现可核验的腾讯分分彩28官方开奖源",
            )),
            _ => self.fetch_legacy_network(game).await,
        }
    }

    async fn fetch_bclc(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        let response = self
            .client
            .get(BCLC_KENO_CURRENT_YEAR_URL)
            .header("Accept", "application/zip")
            .timeout(self.request_timeout.min(Duration::from_secs(6)))
            .send()
            .await
            .map_err(|_| {
                AppError::new("prediction_public_request", "BCLC 官方 Keno 数据连接失败")
            })?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "prediction_public_http",
                format!("BCLC 官方 Keno 返回 HTTP {}", response.status()),
            ));
        }
        let bytes =
            read_capped_body(response, PUBLIC_RESPONSE_LIMIT, "BCLC 官方 Keno 文件").await?;
        let snapshot = parse_bclc_zip(game, &bytes).ok_or_else(|| {
            AppError::new(
                "prediction_public_contract",
                "BCLC 官方 Keno 文件格式无法识别",
            )
        })?;
        Ok(PredictionSourceResult::from_snapshot(
            snapshot,
            Utc::now(),
            self.stale_after,
        ))
    }

    async fn fetch_cwl(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        let response = self
            .client
            .get(CWL_KL8_URL)
            .header("Accept", "application/json")
            .header("Referer", "https://www.cwl.gov.cn/")
            .timeout(self.request_timeout.min(Duration::from_secs(12)))
            .query(&[
                ("name", "kl8"),
                ("issueCount", "30"),
                ("pageNo", "1"),
                ("pageSize", "30"),
            ])
            .send()
            .await
            .map_err(|_| {
                AppError::new("prediction_public_request", "中国福彩网公开数据连接失败")
            })?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "prediction_public_http",
                format!("中国福彩网公开数据返回 HTTP {}", response.status()),
            ));
        }
        let bytes =
            read_capped_body(response, PUBLIC_RESPONSE_LIMIT, "中国福彩网公开数据").await?;
        let snapshot = parse_cwl_snapshot(game, &bytes).ok_or_else(|| {
            AppError::new(
                "prediction_public_contract",
                "中国福彩网快乐8返回结构无法识别",
            )
        })?;
        Ok(PredictionSourceResult::from_snapshot(
            snapshot,
            Utc::now(),
            Duration::from_secs(36 * 60 * 60),
        ))
    }

    async fn fetch_public_keno(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        let response = self
            .client
            .get(PUBLIC_KENO_FALLBACK_URL)
            .header("Accept", "application/json")
            .timeout(self.request_timeout.min(Duration::from_secs(12)))
            .query(&[("nbr", "60")])
            .send()
            .await
            .map_err(|_| {
                AppError::new(
                    "prediction_public_request",
                    "BCLC 官方文件和公开 Keno 回退源都无法连接",
                )
            })?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "prediction_public_http",
                format!("公开 Keno 回退源返回 HTTP {}", response.status()),
            ));
        }
        let bytes =
            read_capped_body(response, PUBLIC_RESPONSE_LIMIT, "公开 Keno 回退源").await?;
        let snapshot = parse_public_keno_snapshot(game, &bytes).ok_or_else(|| {
            AppError::new("prediction_public_contract", "公开 Keno 回退源格式无法识别")
        })?;
        Ok(PredictionSourceResult::from_snapshot(
            snapshot,
            Utc::now(),
            self.stale_after,
        ))
    }

    async fn fetch_legacy_network(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        if self.token.trim().is_empty() {
            return Err(AppError::new(
                "prediction_not_configured",
                "开奖数据源尚未配置独立凭据，预测应用保持停用",
            ));
        }
        let mut last_error = None;
        for base in self.bases().await {
            let response = match self
                .client
                .get(format!("{base}/api/datas/index"))
                .query(&[("token", self.token.trim()), ("name", game.id)])
                .send()
                .await
            {
                Ok(response) => response,
                Err(_) => {
                    last_error = Some(AppError::new(
                        "prediction_request",
                        "开奖数据服务连接失败，正在尝试备用服务",
                    ));
                    continue;
                }
            };
            if !response.status().is_success() {
                last_error = Some(AppError::new(
                    "prediction_http",
                    format!("开奖数据服务返回 HTTP {}", response.status()),
                ));
                continue;
            }
            let bytes = match read_capped_body(response, PUBLIC_RESPONSE_LIMIT, "开奖数据响应").await
            {
                Ok(bytes) => bytes,
                Err(_) => {
                    last_error = Some(AppError::new("prediction_response", "开奖数据响应读取失败"));
                    continue;
                }
            };
            if token_rejected(&bytes) {
                return Err(AppError::new("prediction_auth", "开奖数据凭据无效或已过期"));
            }
            let Some(snapshot) = parse_zcg_snapshot(game, &bytes) else {
                last_error = Some(AppError::new(
                    "prediction_contract",
                    "开奖数据结构已变化，预测暂时停用",
                ));
                continue;
            };
            return Ok(PredictionSourceResult::from_snapshot(
                snapshot,
                Utc::now(),
                self.stale_after,
            ));
        }
        Err(last_error
            .unwrap_or_else(|| AppError::new("prediction_unavailable", "开奖数据暂不可用"))
            .retryable())
    }
}

#[async_trait]
impl PredictionSource for PublicLotterySource {
    async fn fetch_live(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        if let Some(cached) = self.live_cache.lock().await.get(game.id).cloned() {
            if cached.stored_at.elapsed() < live_cache_ttl(game.id) {
                return Ok(cached.result);
            }
        }
        let request_lock = self.request_lock_for(game.id).await;
        let _game_request = request_lock.lock().await;
        if let Some(cached) = self.live_cache.lock().await.get(game.id).cloned() {
            if cached.stored_at.elapsed() < live_cache_ttl(game.id) {
                return Ok(cached.result);
            }
        }
        let _request_permit = self.request_gate.acquire().await.map_err(|_| {
            AppError::new("prediction_shutdown", "开奖数据请求队列已关闭").retryable()
        })?;
        let result = self.fetch_network(game).await?;
        let cached = CachedPrediction {
            stored_at: Instant::now(),
            result: result.clone(),
        };
        self.live_cache
            .lock()
            .await
            .insert(game.id.to_string(), cached.clone());
        self.history_cache
            .lock()
            .await
            .insert(game.id.to_string(), cached);
        Ok(result)
    }

    async fn fetch_history(&self, game: &Game, limit: usize) -> AppResult<Vec<Vec<i64>>> {
        if let Some(cached) = self.history_cache.lock().await.get(game.id).cloned() {
            if cached.stored_at.elapsed() < history_cache_ttl(game.id) {
                return Ok(cached
                    .result
                    .snapshot
                    .map(|snapshot| snapshot.history.into_iter().take(limit).collect())
                    .unwrap_or_default());
            }
        }
        Ok(self
            .fetch_live(game)
            .await?
            .snapshot
            .map(|snapshot| snapshot.history.into_iter().take(limit).collect())
            .unwrap_or_default())
    }
}

fn live_cache_ttl(game_id: &str) -> Duration {
    match game_id {
        // The official BCLC archive is substantially larger than the JSON feeds and
        // Keno draws are several minutes apart, so a one-minute cache is sufficient.
        "jnd" => Duration::from_secs(60),
        // China Welfare Lottery Keno is a daily draw. A short background refresh is
        // still useful around draw time without repeatedly loading the public API.
        "pcdd" | "bj28" => Duration::from_secs(30 * 60),
        _ => Duration::from_secs(10),
    }
}

fn history_cache_ttl(game_id: &str) -> Duration {
    match game_id {
        "jnd" => Duration::from_secs(3 * 60),
        "pcdd" | "bj28" => Duration::from_secs(60 * 60),
        _ => Duration::from_secs(60),
    }
}

#[cfg(any(feature = "fixture", test))]
#[derive(Debug, Clone)]
pub struct FixturePredictionSource {
    snapshots: HashMap<String, PredictionSnapshot>,
    now: DateTime<Utc>,
    stale_after: Duration,
}

#[cfg(any(feature = "fixture", test))]
impl FixturePredictionSource {
    pub fn new(snapshots: impl IntoIterator<Item = PredictionSnapshot>) -> Self {
        let snapshots = snapshots
            .into_iter()
            .map(|snapshot| (snapshot.game.to_lowercase(), snapshot))
            .collect();
        Self {
            snapshots,
            now: Utc::now(),
            stale_after: Duration::from_secs(15 * 60),
        }
    }

    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    pub fn with_stale_after(mut self, stale_after: Duration) -> Self {
        self.stale_after = stale_after;
        self
    }
}

#[async_trait]
#[cfg(any(feature = "fixture", test))]
impl PredictionSource for FixturePredictionSource {
    async fn fetch_live(&self, game: &Game) -> AppResult<PredictionSourceResult> {
        let snapshot = self
            .snapshots
            .get(&game.id.to_lowercase())
            .or_else(|| self.snapshots.get(&game.name.to_lowercase()))
            .cloned();
        Ok(match snapshot {
            Some(snapshot) => {
                PredictionSourceResult::from_snapshot(snapshot, self.now, self.stale_after)
            }
            None => PredictionSourceResult::missing(),
        })
    }
}

fn requested_game<'a>(games: &'a [Game], text: &str) -> Option<&'a Game> {
    let normalized = text.to_lowercase();
    games.iter().find(|game| {
        normalized.contains(&game.name.to_lowercase())
            || normalized.contains(game.id)
            || game
                .aliases
                .iter()
                .any(|alias| normalized.contains(&alias.to_lowercase()))
    })
}

pub async fn fetch_status<S: PredictionSource + ?Sized>(
    text: &str,
    source: &S,
) -> AppResult<Result<PredictionSourceResult, String>> {
    let games = source.list_games().await?;
    let game = requested_game(&games, text);
    let Some(game) = game else {
        return Ok(Err(format!(
            "请在“预测”后写出彩种名称，例如：@DH 预测 {}",
            games
                .iter()
                .map(|game| game.name)
                .collect::<Vec<_>>()
                .join("、")
        )));
    };
    Ok(Ok(source.load(game).await?))
}

pub fn format_reply(result: &PredictionResult) -> String {
    let total = result.latest_result.iter().sum::<i64>();
    format!(
        "【{}】第{}期\n开奖：{} = {}（{}）\n时间：{}\n趋势：{}\n参考方向：{}\n参考度：{}\n说明：基于已核验历史数据统计，仅作信息参考。",
        result.game,
        result.period,
        join(&result.latest_result, " + "),
        total,
        result_shape(total, &result.latest_result),
        result.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
        result.trend,
        if result.candidates.is_empty() {
            "暂无".into()
        } else {
            join(&result.candidates, "、")
        },
        confidence_text(result.confidence),
    )
}

pub fn format_stale_reply(snapshot: &PredictionSnapshot) -> String {
    let total = snapshot.result.iter().sum::<i64>();
    format!(
        "【{}】第{}期\n最后结果：{} = {}（{}）\n时间：{}\n数据状态：结果已过期，仅展示最后一次已核验记录，不生成参考方向。",
        snapshot.game,
        snapshot.period,
        join(&snapshot.result, " + "),
        total,
        result_shape(total, &snapshot.result),
        snapshot.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
    )
}

fn result_shape(total: i64, balls: &[i64]) -> String {
    let size = if total >= 14 { "大" } else { "小" };
    let parity = if total.rem_euclid(2) == 0 {
        "双"
    } else {
        "单"
    };
    let extra = if balls.len() == 3 && balls.iter().all(|value| *value == balls[0]) {
        " · 豹子"
    } else if total <= 5 {
        " · 极小"
    } else if total >= 22 {
        " · 极大"
    } else {
        ""
    };
    format!("{size}{parity}{extra}")
}

#[cfg(test)]
pub fn parse_snapshot(game: &str, body: &[u8]) -> PredictionSnapshot {
    let text = String::from_utf8_lossy(body);
    let pattern = Regex::new(r"(?im)(?:期号|期数|period|issue|expect)[^0-9]{0,12}([0-9]{2,20})[^0-9]{0,20}([0-9]{1,3})[+,:，、 ]+([0-9]{1,3})[+,:，、 ]+([0-9]{1,3})").unwrap();
    let mut period = String::new();
    let mut result = Vec::new();
    let mut history = Vec::new();
    for capture in pattern.captures_iter(&text) {
        let row = (2..=4)
            .filter_map(|index| capture.get(index)?.as_str().parse::<i64>().ok())
            .collect::<Vec<_>>();
        if row.len() != 3 {
            continue;
        }
        if period.is_empty() {
            period = capture
                .get(1)
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            result = row.clone();
        }
        history.push(row);
    }
    if period.is_empty() {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
            period = json_text(&value, &["period", "issue", "expect", "期号", "期数"]);
            result = numbers(&json_text(
                &value,
                &["result", "numbers", "开奖号码", "openCode"],
            ));
            if !result.is_empty() {
                history.push(result.clone());
            }
        }
    }
    PredictionSnapshot {
        game: game.into(),
        period: if period.is_empty() {
            "待更新".into()
        } else {
            period
        },
        result,
        updated_at: Utc.timestamp_opt(0, 0).single().unwrap(),
        history,
        freshness: PredictionFreshness::Fresh.as_str().into(),
    }
}

#[derive(Debug, Clone)]
struct ZcgDraw {
    period: String,
    result: Vec<i64>,
    opened_at: DateTime<Utc>,
}

/// Human-readable source description used by the health page and reply formatter.
pub fn public_source_label(game_id: &str) -> &'static str {
    match game_id {
        "jnd" => "BCLC 官方 Keno 原始开奖；失败时用公开 Keno 回退，DH 派生 28 结果",
        "pcdd" | "bj28" => "中国福彩网官方快乐8开奖，DH 按公开规则派生 28 结果",
        "btc28" => "公开区块数据，但28派生算法未核验",
        "tx28" => "没有可核验的官方公开数据源",
        _ => "兼容数据源",
    }
}

fn three_ball_sum(numbers: &[i64], indexes: &[&[usize]]) -> Option<Vec<i64>> {
    if numbers.len() < 19 {
        return None;
    }
    Some(
        indexes
            .iter()
            .map(|positions| {
                positions
                    .iter()
                    .filter_map(|position| numbers.get(*position))
                    .sum::<i64>()
                    .rem_euclid(10)
            })
            .collect(),
    )
}

/// Canada 28: sorted BCLC Keno positions 2/5/8..., 3/6/9..., 4/7/10...
fn derive_canada28(numbers: &[i64]) -> Option<Vec<i64>> {
    three_ball_sum(
        numbers,
        &[
            &[1, 4, 7, 10, 13, 16],
            &[2, 5, 8, 11, 14, 17],
            &[3, 6, 9, 12, 15, 18],
        ],
    )
}

/// PC28/Beijing 28: sorted 快乐8 positions 1-6, 7-12, 13-18.
fn derive_welfare28(numbers: &[i64]) -> Option<Vec<i64>> {
    three_ball_sum(
        numbers,
        &[
            &[0, 1, 2, 3, 4, 5],
            &[6, 7, 8, 9, 10, 11],
            &[12, 13, 14, 15, 16, 17],
        ],
    )
}

fn snapshot_from_draws(game: &Game, mut draws: Vec<ZcgDraw>) -> Option<PredictionSnapshot> {
    draws.retain(|draw| !draw.period.is_empty() && draw.result.len() == 3);
    draws.sort_by(|left, right| period_cmp(&right.period, &left.period));
    let mut periods = HashSet::new();
    draws.retain(|draw| periods.insert(draw.period.clone()));
    let latest = draws.first()?.clone();
    Some(PredictionSnapshot {
        game: game.name.into(),
        period: latest.period,
        result: latest.result,
        updated_at: latest.opened_at,
        history: draws.into_iter().take(60).map(|draw| draw.result).collect(),
        freshness: PredictionFreshness::Fresh.as_str().into(),
    })
}

fn parse_bclc_zip(game: &Game, body: &[u8]) -> Option<PredictionSnapshot> {
    let mut archive = ZipArchive::new(Cursor::new(body)).ok()?;
    let mut csv = String::new();
    archive
        .by_name("KenoCurrentYear.csv")
        .ok()?
        .read_to_string(&mut csv)
        .ok()?;
    parse_bclc_csv(game, &csv)
}

fn parse_bclc_csv(game: &Game, csv: &str) -> Option<PredictionSnapshot> {
    let mut draws = Vec::new();
    for line in csv.lines().skip(1) {
        let fields = line
            .split(',')
            .map(|field| field.trim().trim_matches('"'))
            .collect::<Vec<_>>();
        if fields.len() < 24 || fields.first().copied() != Some("KENO") {
            continue;
        }
        let period = fields[1].to_string();
        let Ok(local) = NaiveDateTime::parse_from_str(fields[2], "%Y-%m-%d %H:%M:%S") else {
            continue;
        };
        let opened_at = Vancouver
            .from_local_datetime(&local)
            .single()
            .or_else(|| Vancouver.from_local_datetime(&local).earliest());
        let Some(opened_at) = opened_at else {
            continue;
        };
        let opened_at = opened_at.with_timezone(&Utc);
        let mut numbers = fields[4..24]
            .iter()
            .filter_map(|value| value.parse::<i64>().ok())
            .collect::<Vec<_>>();
        if numbers.len() != 20 {
            continue;
        }
        numbers.sort_unstable();
        let Some(result) = derive_canada28(&numbers) else {
            continue;
        };
        draws.push(ZcgDraw {
            period,
            result,
            opened_at,
        });
    }
    snapshot_from_draws(game, draws)
}

fn parse_cwl_snapshot(game: &Game, body: &[u8]) -> Option<PredictionSnapshot> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let rows = value.get("result")?.as_array()?;
    let mut draws = Vec::new();
    for row in rows {
        let Some(period) = row.get("code").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(date) = row
            .get("date")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.get(..10))
        else {
            continue;
        };
        let Ok(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
            continue;
        };
        let Some(local) = date.and_hms_opt(21, 30, 0) else {
            continue;
        };
        let opened_at = Shanghai
            .from_local_datetime(&local)
            .single()
            .or_else(|| Shanghai.from_local_datetime(&local).earliest());
        let Some(opened_at) = opened_at else {
            continue;
        };
        let opened_at = opened_at.with_timezone(&Utc);
        let Some(red) = row.get("red").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let mut numbers = red
            .split(',')
            .filter_map(|value| value.trim().parse::<i64>().ok())
            .collect::<Vec<_>>();
        if numbers.len() != 20 {
            continue;
        }
        numbers.sort_unstable();
        let Some(result) = derive_welfare28(&numbers) else {
            continue;
        };
        draws.push(ZcgDraw {
            period: period.to_string(),
            result,
            opened_at,
        });
    }
    snapshot_from_draws(game, draws)
}

fn parse_public_keno_snapshot(game: &Game, body: &[u8]) -> Option<PredictionSnapshot> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let rows = value.get("data")?.as_array()?;
    let mut draws = Vec::new();
    for row in rows {
        // 单行字段缺失只跳过该行，不能让整个回退载荷失败——与 parse_bclc_csv /
        // parse_cwl_snapshot 的行级容错保持一致。
        let (Some(period), Some(date), Some(time), Some(raw_numbers)) = (
            row.get("nbr").and_then(|value| value.as_str()),
            row.get("date").and_then(|value| value.as_str()),
            row.get("time").and_then(|value| value.as_str()),
            row.get("nbrs").and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        let Ok(local) =
            NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M:%S")
        else {
            continue;
        };
        // The public mirror normalizes its date/time fields to Beijing time.
        let Some(opened_at) = Shanghai
            .from_local_datetime(&local)
            .single()
            .or_else(|| Shanghai.from_local_datetime(&local).earliest())
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };
        let mut numbers = raw_numbers
            .split(',')
            .filter_map(|value| value.trim().parse::<i64>().ok())
            .collect::<Vec<_>>();
        if numbers.len() != 20 {
            continue;
        }
        numbers.sort_unstable();
        let Some(result) = derive_canada28(&numbers) else {
            continue;
        };
        draws.push(ZcgDraw {
            period: period.to_string(),
            result,
            opened_at,
        });
    }
    snapshot_from_draws(game, draws)
}

fn token_rejected(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_lowercase();
    (text.contains("token") || text.contains("令牌"))
        && (text.contains("错误")
            || text.contains("无效")
            || text.contains("失败")
            || text.contains("expired")
            || text.contains("invalid"))
}

fn parse_zcg_snapshot(game: &Game, body: &[u8]) -> Option<PredictionSnapshot> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let mut draws = Vec::new();
    collect_zcg_draws(&value, &mut draws);
    snapshot_from_draws(game, draws)
}

fn collect_zcg_draws(value: &serde_json::Value, output: &mut Vec<ZcgDraw>) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_zcg_draws(value, output);
            }
        }
        serde_json::Value::Object(values) => {
            let period = json_text(
                value,
                &["issue", "expect", "preDrawIssue", "full_expect", "period"],
            );
            let code = json_text(
                value,
                &[
                    "kjcodes",
                    "opencode",
                    "preDrawCode",
                    "open_code",
                    "kjcode",
                    "result",
                ],
            );
            let opened_at = ["time", "opentime", "preDrawTime", "open_time", "openTime"]
                .iter()
                .find_map(|key| values.get(*key).and_then(parse_open_time));
            if !period.is_empty() && !code.is_empty() {
                if let Some(opened_at) = opened_at {
                    let result = numbers(&code);
                    if result.len() == 3 {
                        output.push(ZcgDraw {
                            period,
                            result,
                            opened_at,
                        });
                    }
                }
            }
            for nested in values.values() {
                if nested.is_array() || nested.is_object() {
                    collect_zcg_draws(nested, output);
                }
            }
        }
        _ => {}
    }
}

fn parse_open_time(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    if let Some(value) = value.as_i64() {
        let seconds = if value > 10_000_000_000 {
            value / 1000
        } else {
            value
        };
        return Utc.timestamp_opt(seconds, 0).single();
    }
    let text = value.as_str()?.trim();
    if let Ok(value) = DateTime::parse_from_rfc3339(text) {
        return Some(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y/%m/%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(text, format) {
            return Some(Utc.from_utc_datetime(&value));
        }
    }
    text.parse::<i64>()
        .ok()
        .and_then(|value| parse_open_time(&serde_json::Value::from(value)))
}

fn period_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    match (left.parse::<u128>(), right.parse::<u128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn json_text(value: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = value.get(*key) {
            if let Some(text) = value.as_str() {
                return text.into();
            }
            if !value.is_null() {
                return value.to_string().trim_matches('"').into();
            }
        }
    }
    String::new()
}
fn numbers(value: &str) -> Vec<i64> {
    Regex::new(r"[0-9]{1,3}")
        .unwrap()
        .find_iter(value)
        .filter_map(|item| item.as_str().parse().ok())
        .take(3)
        .collect()
}
fn join(values: &[i64], separator: &str) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}
fn confidence_text(value: f64) -> &'static str {
    if value >= 0.65 {
        "中高"
    } else if value >= 0.5 {
        "中"
    } else {
        "低"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    #[test]
    fn prediction_requires_explicit_mention() {
        assert!(is_prediction_request("@DH 预测 PC28"));
        assert!(!is_prediction_request("预测 PC28"));
    }
    #[test]
    fn analysis_is_deterministic() {
        let snapshot = PredictionSnapshot {
            game: "PC28".into(),
            period: "1".into(),
            result: vec![1, 2, 3],
            updated_at: Utc::now(),
            history: vec![vec![1, 2, 3], vec![1, 4, 5], vec![2, 6, 7]],
            freshness: "fresh".into(),
        };
        assert_eq!(analyze(&snapshot).candidates, analyze(&snapshot).candidates);
    }
    #[test]
    fn fixture_parser_hides_source_shape() {
        let snapshot = parse_snapshot(
            "PC28",
            "期号 20260720001 1+2+3 期号 20260720000 4+5+6".as_bytes(),
        );
        assert_eq!(snapshot.period, "20260720001");
        assert_eq!(snapshot.history.len(), 2);
        assert!(!format_reply(&analyze(&snapshot)).contains("http"));
    }

    fn snapshot(updated_at: DateTime<Utc>) -> PredictionSnapshot {
        PredictionSnapshot {
            game: "PC28".into(),
            period: "20260721001".into(),
            result: vec![1, 2, 3],
            updated_at,
            history: vec![vec![1, 2, 3]; 10],
            freshness: "fresh".into(),
        }
    }

    #[test]
    fn freshness_boundary_and_missing_source_are_explicit() {
        let now = Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, 0).unwrap();
        assert_eq!(
            freshness_status(
                now - chrono::Duration::minutes(15),
                now,
                Duration::from_secs(15 * 60)
            ),
            PredictionFreshness::Fresh
        );
        assert_eq!(
            freshness_status(
                now - chrono::Duration::seconds(901),
                now,
                Duration::from_secs(15 * 60)
            ),
            PredictionFreshness::Stale
        );
        assert_eq!(
            PredictionSourceResult::missing().status,
            PredictionFreshness::Missing
        );
    }

    #[tokio::test]
    async fn fixture_source_reports_stale_and_missing_games() {
        let now = Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, 0).unwrap();
        let source = FixturePredictionSource::new([snapshot(now - chrono::Duration::minutes(30))])
            .with_now(now)
            .with_stale_after(Duration::from_secs(15 * 60));
        let pc28 = GAMES.iter().find(|game| game.id == "pcdd").unwrap();
        let missing = GAMES.iter().find(|game| game.id == "tx28").unwrap();
        assert_eq!(
            source.load(pc28).await.unwrap().status,
            PredictionFreshness::Stale
        );
        assert_eq!(
            source.load(missing).await.unwrap().status,
            PredictionFreshness::Missing
        );
    }

    #[test]
    fn formatted_reply_excludes_upstream_shape_and_credentials() {
        let body = br#"{
            "period":"20260721001",
            "result":"1,2,3",
            "provider":"UPSTREAM_VENDOR",
            "token":"SECRET_TOKEN",
            "endpoint":"https://upstream.invalid/raw"
        }"#;
        let reply = format_reply(&analyze(&parse_snapshot("PC28", body)));
        for hidden in [
            "UPSTREAM_VENDOR",
            "SECRET_TOKEN",
            "upstream.invalid",
            "provider",
            "token",
        ] {
            assert!(!reply.contains(hidden), "leaked upstream field: {hidden}");
        }
        assert!(reply.contains("20260721001"));
        assert!(reply.contains("1 + 2 + 3"));
    }

    #[test]
    fn parses_all_known_zcg_contract_shapes_with_real_times() {
        let game = GAMES.iter().find(|game| game.id == "pcdd").unwrap();
        for body in [
            r#"{"datas":[{"issue":"20260728002","kjcodes":"1,2,3","time":"2026-07-28 12:02:00"},{"issue":"20260728001","kjcodes":"4,5,6","time":"2026-07-28 12:01:00"}]}"#,
            r#"{"data":[{"expect":"20260728002","opencode":"1,2,3","opentime":"2026-07-28 12:02:00"}]}"#,
            r#"{"result":{"data":[{"preDrawIssue":"20260728002","preDrawCode":"1,2,3","preDrawTime":"2026-07-28 12:02:00"}]}}"#,
            r#"{"data":[{"full_expect":"20260728002","open_code":"1,2,3","open_time":"2026-07-28 12:02:00"}]}"#,
        ] {
            let snapshot = parse_zcg_snapshot(game, body.as_bytes()).unwrap();
            assert_eq!(snapshot.period, "20260728002");
            assert_eq!(snapshot.result, vec![1, 2, 3]);
            assert_eq!(
                snapshot.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                "2026-07-28 12:02:00"
            );
        }
    }

    #[test]
    fn zcg_contract_rejects_missing_draw_time_and_token_errors() {
        let game = GAMES.iter().find(|game| game.id == "pcdd").unwrap();
        assert!(parse_zcg_snapshot(
            game,
            br#"{"datas":[{"issue":"20260728002","kjcodes":"1,2,3"}]}"#
        )
        .is_none());
        assert!(token_rejected("token验证错误".as_bytes()));
        let source = PublicLotterySource::new(Duration::from_secs(1))
            .unwrap()
            .with_token("TEST_TOKEN");
        assert_eq!(source.token, "TEST_TOKEN");
    }

    #[tokio::test]
    async fn zcg_live_requests_are_single_flight_and_cached() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = hits.clone();
        let opened_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            server_hits.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 8 * 1024];
            let _ = stream.read(&mut request);
            let body = serde_json::json!({
                "data": [{
                    "issue": "20260728001",
                    "kjcodes": "1,2,3",
                    "time": opened_at
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let source = PublicLotterySource::new(Duration::from_secs(1))
            .unwrap()
            .with_token("TEST_TOKEN")
            .with_test_base(format!("http://{address}"));
        let game = GAMES.iter().find(|game| game.id == "btc28").unwrap();
        let (first, second) = tokio::join!(source.fetch_live(game), source.fetch_live(game));
        assert_eq!(first.unwrap().status, PredictionFreshness::Fresh);
        assert_eq!(second.unwrap().status, PredictionFreshness::Fresh);
        assert_eq!(
            source.fetch_live(game).await.unwrap().status,
            PredictionFreshness::Fresh
        );
        worker.join().unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn capped_body_maps_over_limit_to_prediction_error_code() {
        // 读取机制本身由 http_body 模块的测试覆盖；这里只钉住本模块的错误码映射，
        // 避免共享模块的错误类型泄漏成别的域的错误码。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8 * 1024];
            let _ = stream.read(&mut request);
            let body = "x".repeat(16 * 1024);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            // 客户端一超限就断开连接，这里的写入可能是 broken pipe，属预期。
            let _ = stream.write_all(response.as_bytes());
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        let error = read_capped_body(response, 1024, "测试源").await.unwrap_err();
        assert_eq!(error.code, "prediction_public_response");
        assert_eq!(error.message, "测试源超过大小限制");
        worker.join().unwrap();
    }

    #[test]
    fn derives_canada_28_from_sorted_bclc_keno_numbers() {
        let values = vec![
            5, 11, 12, 14, 15, 16, 17, 18, 22, 23, 29, 30, 37, 44, 47, 48, 51, 76, 78, 79,
        ];
        assert_eq!(derive_canada28(&values), Some(vec![8, 3, 7]));
    }

    #[test]
    fn derives_welfare_28_from_official_kl8_numbers() {
        let values = vec![
            1, 4, 7, 10, 12, 15, 16, 17, 26, 29, 32, 40, 42, 44, 49, 65, 68, 73, 77, 78,
        ];
        assert_eq!(derive_welfare28(&values), Some(vec![9, 0, 1]));
    }

    #[test]
    fn parses_bclc_csv_and_preserves_official_draw_time() {
        let game = GAMES.iter().find(|game| game.id == "jnd").unwrap();
        let csv = concat!(
            "\"PRODUCT\",\"DRAW NUMBER\",\"DRAW DATE\",\"BONUS MULTIPLIER\",\"NUMBER DRAWN 1\"\n",
            "\"KENO\",3463830,\"2026-07-31 03:56:30\",1,5,11,12,14,15,16,17,18,22,23,29,30,37,44,47,48,51,76,78,79\n",
        );
        let snapshot = parse_bclc_csv(game, csv).unwrap();
        assert_eq!(snapshot.period, "3463830");
        assert_eq!(snapshot.result, vec![8, 3, 7]);
        assert_eq!(
            snapshot.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-07-31 10:56:30"
        );
    }

    #[test]
    fn parses_cwl_kl8_json_and_derives_public_result() {
        let game = GAMES.iter().find(|game| game.id == "pcdd").unwrap();
        let body = r#"{
          "result": [{
            "code": "2026202",
            "date": "2026-07-31(五)",
            "red": "01,04,07,10,12,15,16,17,26,29,32,40,42,44,49,65,68,73,77,78"
          }]
        }"#;
        let snapshot = parse_cwl_snapshot(game, body.as_bytes()).unwrap();
        assert_eq!(snapshot.period, "2026202");
        assert_eq!(snapshot.result, vec![9, 0, 1]);
        assert_eq!(
            snapshot.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-07-31 13:30:00"
        );
    }

    #[test]
    fn parses_public_keno_fallback_without_exposing_provider_fields() {
        let game = GAMES.iter().find(|game| game.id == "jnd").unwrap();
        let body = br#"{
          "countdown":"00:35",
          "data":[{
            "nbr":"3464041",
            "date":"2026-08-01",
            "time":"07:48:00",
            "nbrs":"3,5,6,9,10,18,23,26,27,28,31,39,42,53,56,59,62,63,68,70",
            "bonus":"1"
          }],
          "message":"success"
        }"#;
        let snapshot = parse_public_keno_snapshot(game, body).unwrap();
        assert_eq!(snapshot.period, "3464041");
        assert_eq!(snapshot.result, vec![7, 9, 9]);
        assert_eq!(
            snapshot.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-07-31 23:48:00"
        );
    }
}
