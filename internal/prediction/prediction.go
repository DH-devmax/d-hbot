package prediction

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
)

// Game describes a packaged ZCG-compatible data adapter. The endpoint is kept
// inside the adapter and never exposed to group replies or the desktop UI.
type Game struct {
	ID    string   `json:"id"`
	Name  string   `json:"name"`
	Kind  string   `json:"kind"`
	Alias []string `json:"aliases,omitempty"`
}

type Snapshot struct {
	Game       Game
	Period     string
	Result     []int
	UpdatedAt  time.Time
	History    [][]int
	Statistics Statistics
	Freshness  string
	DataStatus string
}

type Statistics struct {
	Window     int
	Hot        []int
	Cold       []int
	Candidates []int
	Omissions  map[int]int
	Trend      string
	Confidence string
}

type Source interface {
	ListGames(context.Context) ([]Game, error)
	FetchLive(context.Context, Game) (Snapshot, error)
	FetchHistory(context.Context, Game, int) ([][]int, error)
}

type Service struct {
	source Source
}

func New() *Service { return &Service{source: NewBuiltinSource(nil)} }
func NewWithSource(source Source) *Service {
	if source == nil {
		return New()
	}
	return &Service{source: source}
}

func IsRequest(text string) bool {
	return strings.Contains(strings.ToLower(text), "预测") && hasMention(text)
}

func hasMention(text string) bool {
	lower := strings.ReplaceAll(strings.ToLower(text), "@ dh", "@dh")
	for offset := 0; offset < len(lower); {
		index := strings.Index(lower[offset:], "@dh")
		if index < 0 {
			return false
		}
		index += offset
		end := index + 3
		if end < len(lower) && (lower[end] == ' ' || lower[end] == '\t') {
			for end < len(lower) && (lower[end] == ' ' || lower[end] == '\t') {
				end++
			}
		}
		if end == len(lower) || !isWordContinuation(lower[end]) {
			return true
		}
		offset = end
	}
	return false
}

func isWordContinuation(value byte) bool {
	return value >= 'a' && value <= 'z' || value >= '0' && value <= '9' || value == '_'
}

func (s *Service) Build(ctx context.Context, text string) (Snapshot, string, error) {
	if s == nil || s.source == nil {
		return Snapshot{}, "", errors.New("预测数据源未配置")
	}
	games, err := s.source.ListGames(ctx)
	if err != nil {
		return Snapshot{}, "", err
	}
	game := chooseGame(text, games)
	if game.ID == "" {
		return Snapshot{}, gameList(games), nil
	}
	snapshot, err := s.source.FetchLive(ctx, game)
	if err != nil {
		return Snapshot{}, "", err
	}
	if len(snapshot.History) == 0 {
		snapshot.History, _ = s.source.FetchHistory(ctx, game, 20)
	}
	snapshot.Statistics = Analyze(snapshot.History)
	return snapshot, prompt(snapshot), nil
}

func chooseGame(text string, games []Game) Game {
	lower := strings.ToLower(text)
	for _, game := range games {
		if strings.Contains(lower, strings.ToLower(game.Name)) || strings.Contains(lower, strings.ToLower(game.ID)) {
			return game
		}
		for _, alias := range game.Alias {
			if strings.Contains(lower, strings.ToLower(alias)) {
				return game
			}
		}
	}
	if len(games) == 1 {
		return games[0]
	}
	return Game{}
}

func gameList(games []Game) string {
	if len(games) == 0 {
		return "当前没有可识别的彩种数据。"
	}
	names := make([]string, 0, len(games))
	for _, game := range games {
		names = append(names, game.Name)
	}
	sort.Strings(names)
	return "请在“预测”后写出彩种名称，例如：@DH 预测 " + strings.Join(names, "、")
}

func prompt(snapshot Snapshot) string {
	parts := make([]string, 0, len(snapshot.Result))
	for _, value := range snapshot.Result {
		parts = append(parts, strconv.Itoa(value))
	}
	stats := snapshot.Statistics
	return fmt.Sprintf("请根据 DH BOT 已整理的统计结果生成自然简洁的中文回复。彩种=%s；期号=%s；最新结果=%s；更新时间=%s；数据状态=%s；统计窗口=%d；热码=%s；冷码=%s；候选方向=%s；趋势=%s；参考度=%s。回复只展示彩种、期号、最新结果、更新时间、趋势摘要、候选方向和参考度，不提接口、供应方或原始字段。", snapshot.Game.Name, snapshot.Period, strings.Join(parts, ","), snapshot.UpdatedAt.Local().Format("2006-01-02 15:04:05"), snapshot.DataStatus, stats.Window, joinInts(stats.Hot), joinInts(stats.Cold), joinInts(stats.Candidates), stats.Trend, stats.Confidence)
}

func Fallback(snapshot Snapshot) string {
	result := make([]string, 0, len(snapshot.Result))
	for _, value := range snapshot.Result {
		result = append(result, strconv.Itoa(value))
	}
	when := "时间未知"
	if !snapshot.UpdatedAt.IsZero() {
		when = snapshot.UpdatedAt.Local().Format("01-02 15:04")
	}
	stats := snapshot.Statistics
	return fmt.Sprintf("%s 第%s期\n最新结果：%s\n更新时间：%s\n数据状态：%s\n趋势摘要：%s\n候选方向：%s\n参考度：%s，仅作信息参考。", snapshot.Game.Name, snapshot.Period, strings.Join(result, " + "), when, firstNonEmpty(snapshot.DataStatus, "已整理"), firstNonEmpty(stats.Trend, "样本不足，暂不判断冷热"), firstNonEmpty(joinInts(stats.Candidates), "暂无"), firstNonEmpty(stats.Confidence, "低"))
}

// Analyze calculates a deterministic summary before AI wording. It deliberately
// exposes only normalized statistics, never the source response shape.
func Analyze(history [][]int) Statistics {
	if len(history) > 30 {
		history = history[:30]
	}
	stats := Statistics{Window: len(history), Omissions: make(map[int]int), Confidence: "低"}
	counts := make(map[int]int)
	for _, row := range history {
		for _, value := range row {
			if value < 0 || value > 99 {
				continue
			}
			counts[value]++
		}
	}
	const maxResult = 27
	for value := 0; value <= maxResult; value++ {
		miss := len(history)
		for index, row := range history {
			if containsInt(row, value) {
				miss = index
				break
			}
		}
		stats.Omissions[value] = miss
	}
	values := make([]int, maxResult+1)
	for value := range values {
		values[value] = value
	}
	sort.SliceStable(values, func(i, j int) bool {
		if counts[values[i]] == counts[values[j]] {
			return values[i] < values[j]
		}
		return counts[values[i]] > counts[values[j]]
	})
	stats.Hot = append([]int(nil), values[:3]...)
	sort.SliceStable(values, func(i, j int) bool {
		if counts[values[i]] == counts[values[j]] {
			return stats.Omissions[values[i]] > stats.Omissions[values[j]]
		}
		return counts[values[i]] < counts[values[j]]
	})
	stats.Cold = append([]int(nil), values[:3]...)
	sort.SliceStable(values, func(i, j int) bool {
		left := counts[values[i]]*2 + minInt(stats.Omissions[values[i]], 10)
		right := counts[values[j]]*2 + minInt(stats.Omissions[values[j]], 10)
		if left == right {
			return values[i] < values[j]
		}
		return left > right
	})
	stats.Candidates = append([]int(nil), values[:3]...)
	if stats.Window == 0 {
		stats.Hot, stats.Cold, stats.Candidates = nil, nil, nil
		stats.Trend = "样本不足，暂不判断冷热"
		return stats
	}
	stats.Trend = fmt.Sprintf("近%d期热码%s，冷码%s", stats.Window, joinInts(stats.Hot), joinInts(stats.Cold))
	if stats.Window >= 10 {
		stats.Confidence = "中"
	}
	if stats.Window >= 25 {
		stats.Confidence = "中高"
	}
	return stats
}

func containsInt(values []int, target int) bool {
	for _, value := range values {
		if value == target {
			return true
		}
	}
	return false
}

func minInt(left, right int) int {
	if left < right {
		return left
	}
	return right
}

func joinInts(values []int) string {
	parts := make([]string, 0, len(values))
	for _, value := range values {
		parts = append(parts, strconv.Itoa(value))
	}
	return strings.Join(parts, "、")
}

// BuiltinSource keeps the ZCG-compatible adapter list in the executable while
// allowing tests to inject a deterministic HTTP client or fixture.
type BuiltinSource struct {
	client    *http.Client
	games     []Game
	endpoints map[string]string
}

func NewBuiltinSource(client *http.Client) *BuiltinSource {
	if client == nil {
		client = &http.Client{Timeout: 8 * time.Second}
	}
	return &BuiltinSource{client: client, games: []Game{
		{ID: "pc28", Name: "PC28", Kind: "三位数", Alias: []string{"pc蛋蛋", "PC蛋蛋"}},
		{ID: "jnd28", Name: "加拿大28", Kind: "三位数", Alias: []string{"加拿大"}},
		{ID: "bj28", Name: "北京28", Kind: "三位数", Alias: []string{"北京"}},
		{ID: "bit", Name: "比特", Kind: "三位数", Alias: []string{"比特彩"}},
	}, endpoints: map[string]string{
		"pc28":  "http://www.pceggs.com/play/pc28.aspx",
		"jnd28": "http://www.ok1116.com/play/jnd28/",
		"bj28":  "https://hao123wc.obs.ap-southeast-1.myhuaweicloud.com/zcs/duoduo_2.txt",
	}}
}

func (s *BuiltinSource) ListGames(context.Context) ([]Game, error) {
	return append([]Game(nil), s.games...), nil
}

func (s *BuiltinSource) FetchLive(ctx context.Context, game Game) (Snapshot, error) {
	endpoint := s.endpoints[game.ID]
	if endpoint == "" {
		return Snapshot{}, errors.New("当前彩种暂未发现可用数据")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return Snapshot{}, err
	}
	response, err := s.client.Do(req)
	if err != nil {
		return Snapshot{}, fmt.Errorf("读取预测数据失败: %w", err)
	}
	defer response.Body.Close()
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return Snapshot{}, fmt.Errorf("预测数据暂不可用（HTTP %d）", response.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(response.Body, 2<<20))
	if err != nil {
		return Snapshot{}, err
	}
	return parseSnapshot(game, body, time.Now()), nil
}

func (s *BuiltinSource) FetchHistory(ctx context.Context, game Game, limit int) ([][]int, error) {
	snapshot, err := s.FetchLive(ctx, game)
	if err != nil {
		return nil, err
	}
	return snapshot.History, nil
}

var numberPattern = regexp.MustCompile(`(?m)(?:期号|期数|period|issue|expect)[^0-9]{0,12}([0-9]{2,20})[^0-9]{0,20}([0-9]{1,3})[+,:，、 ]+([0-9]{1,3})[+,:，、 ]+([0-9]{1,3})`)

func parseSnapshot(game Game, body []byte, now time.Time) Snapshot {
	text := strings.TrimSpace(string(body))
	var generic map[string]any
	_ = json.Unmarshal(body, &generic)
	period := firstJSON(generic, "period", "issue", "expect", "期号", "期数")
	values := []int{}
	if raw := firstJSON(generic, "result", "numbers", "开奖号码", "openCode"); raw != "" {
		values = parseNumbers(raw)
	}
	history := make([][]int, 0)
	for _, match := range numberPattern.FindAllStringSubmatch(text, -1) {
		if len(match) != 5 {
			continue
		}
		row := parseNumbers(strings.Join(match[2:], ","))
		if len(row) < 3 {
			continue
		}
		if period == "" {
			period = match[1]
		}
		if len(values) == 0 {
			values = append([]int(nil), row...)
		}
		history = append(history, row)
	}
	if len(values) == 0 && !strings.Contains(strings.ToLower(text), "<html") {
		values = parseNumbers(text)
		if len(values) > 3 {
			values = values[:3]
		}
	}
	status := "已整理"
	if len(values) == 0 {
		status = "未解析到最新结果"
	}
	return Snapshot{Game: game, Period: firstNonEmpty(period, "待更新"), Result: values, UpdatedAt: now, History: history, Statistics: Analyze(history), Freshness: "刚刚", DataStatus: status}
}

func parseNumbers(value string) []int {
	parts := regexp.MustCompile(`[0-9]{1,3}`).FindAllString(value, -1)
	out := make([]int, 0, len(parts))
	for _, part := range parts {
		value, err := strconv.Atoi(part)
		if err == nil && value <= 999 {
			out = append(out, value)
		}
	}
	return out
}

func firstJSON(values map[string]any, keys ...string) string {
	for _, key := range keys {
		if value, ok := values[key]; ok {
			switch typed := value.(type) {
			case string:
				if strings.TrimSpace(typed) != "" {
					return strings.TrimSpace(typed)
				}
			default:
				return fmt.Sprint(typed)
			}
		}
	}
	return ""
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return strings.TrimSpace(value)
		}
	}
	return ""
}
