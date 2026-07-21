package store

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
	"time"

	"dh/internal/groupmgr"
)

const defaultKnowledgeBaseName = "DH 默认群规与 FAQ"

const defaultKnowledgeContent = `群内请文明交流，不发布骚扰、欺诈、恶意链接或诱导性内容。
涉及资金、账号、验证码和身份信息时，请先人工核实，不要直接相信陌生人的要求。
DH 只依据管理员配置执行，不替代管理员判断；需要确认的事项请联系群管理员。
知识库没有覆盖的问题，应明确说明资料不足，不编造答案。`

func (s *Store) EnsureDefaultKnowledgeBase(ctx context.Context, accountID string) (groupmgr.KnowledgeBase, error) {
	if strings.TrimSpace(accountID) == "" {
		return groupmgr.KnowledgeBase{}, errors.New("知识库缺少账号")
	}
	var base groupmgr.KnowledgeBase
	err := s.scanKnowledgeBase(s.DB.QueryRowContext(ctx, `SELECT id,account_id,name,description,enabled,built_in,read_only,created_at,updated_at FROM knowledge_bases WHERE account_id=? AND name=?`, accountID, defaultKnowledgeBaseName), &base)
	if err == nil {
		return base, nil
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return base, err
	}
	now := time.Now().UTC()
	base = groupmgr.KnowledgeBase{AccountID: accountID, Name: defaultKnowledgeBaseName, Description: "DH 内置的保守群规与 FAQ，默认不绑定群。", Enabled: true, BuiltIn: true, ReadOnly: true, CreatedAt: now, UpdatedAt: now}
	if err := s.CreateKnowledgeBase(ctx, &base); err != nil {
		return groupmgr.KnowledgeBase{}, err
	}
	doc := groupmgr.KnowledgeDocument{BaseID: base.ID, Title: "默认群规与处理原则", Kind: "faq", Content: defaultKnowledgeContent, Source: "DH 内置", CreatedAt: now, UpdatedAt: now}
	if err := s.insertKnowledgeDocument(ctx, &doc); err != nil {
		return groupmgr.KnowledgeBase{}, err
	}
	return base, nil
}

func (s *Store) CreateKnowledgeBase(ctx context.Context, base *groupmgr.KnowledgeBase) error {
	if strings.TrimSpace(base.Name) == "" {
		return errors.New("知识库名称不能为空")
	}
	now := time.Now().UTC()
	if base.CreatedAt.IsZero() {
		base.CreatedAt = now
	}
	if base.UpdatedAt.IsZero() {
		base.UpdatedAt = now
	}
	result, err := s.DB.ExecContext(ctx, `INSERT INTO knowledge_bases(account_id,name,description,enabled,built_in,read_only,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)`, base.AccountID, strings.TrimSpace(base.Name), base.Description, boolInt(base.Enabled), boolInt(base.BuiltIn), boolInt(base.ReadOnly), nowText(base.CreatedAt), nowText(base.UpdatedAt))
	if err != nil {
		return err
	}
	base.ID, err = result.LastInsertId()
	return err
}

func (s *Store) UpdateKnowledgeBase(ctx context.Context, base *groupmgr.KnowledgeBase) error {
	if base.ID == 0 || strings.TrimSpace(base.Name) == "" {
		return errors.New("知识库信息不完整")
	}
	if base.ReadOnly || base.BuiltIn {
		return errors.New("内置只读知识库不能修改")
	}
	base.UpdatedAt = time.Now().UTC()
	_, err := s.DB.ExecContext(ctx, `UPDATE knowledge_bases SET name=?,description=?,enabled=?,updated_at=? WHERE id=? AND account_id=? AND built_in=0 AND read_only=0`, strings.TrimSpace(base.Name), base.Description, boolInt(base.Enabled), nowText(base.UpdatedAt), base.ID, base.AccountID)
	return err
}

func (s *Store) ListKnowledgeBases(ctx context.Context, accountID string) ([]groupmgr.KnowledgeBase, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,name,description,enabled,built_in,read_only,created_at,updated_at FROM knowledge_bases WHERE account_id=? ORDER BY built_in DESC,name,id`, accountID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.KnowledgeBase
	for rows.Next() {
		var base groupmgr.KnowledgeBase
		if err := s.scanKnowledgeBase(rows, &base); err != nil {
			return nil, err
		}
		out = append(out, base)
	}
	return out, rows.Err()
}

func (s *Store) DeleteKnowledgeBase(ctx context.Context, accountID string, id int64) error {
	var builtIn, readOnly int64
	if err := s.DB.QueryRowContext(ctx, `SELECT built_in,read_only FROM knowledge_bases WHERE id=? AND account_id=?`, id, accountID).Scan(&builtIn, &readOnly); err != nil {
		return err
	}
	if builtIn != 0 || readOnly != 0 {
		return errors.New("内置只读知识库不能删除")
	}
	_, err := s.DB.ExecContext(ctx, `DELETE FROM knowledge_bases WHERE id=? AND account_id=?`, id, accountID)
	return err
}

func (s *Store) UpsertKnowledgeDocument(ctx context.Context, doc *groupmgr.KnowledgeDocument) error {
	if doc.BaseID == 0 || strings.TrimSpace(doc.Title) == "" || strings.TrimSpace(doc.Content) == "" {
		return errors.New("知识库文档信息不完整")
	}
	var readOnly int64
	if err := s.DB.QueryRowContext(ctx, `SELECT read_only FROM knowledge_bases WHERE id=?`, doc.BaseID).Scan(&readOnly); err != nil {
		return err
	}
	if readOnly != 0 {
		return errors.New("内置只读知识库不能修改文档")
	}
	return s.insertOrUpdateKnowledgeDocument(ctx, doc)
}

func (s *Store) insertKnowledgeDocument(ctx context.Context, doc *groupmgr.KnowledgeDocument) error {
	return s.insertOrUpdateKnowledgeDocument(ctx, doc)
}

func (s *Store) insertOrUpdateKnowledgeDocument(ctx context.Context, doc *groupmgr.KnowledgeDocument) error {
	digest := sha256.Sum256([]byte(doc.Content))
	doc.ContentHash = hex.EncodeToString(digest[:])
	now := time.Now().UTC()
	if doc.CreatedAt.IsZero() {
		doc.CreatedAt = now
	}
	doc.UpdatedAt = now
	if doc.ID == 0 {
		result, err := s.DB.ExecContext(ctx, `INSERT INTO knowledge_documents(base_id,title,kind,content,source,content_hash,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)`, doc.BaseID, doc.Title, doc.Kind, doc.Content, doc.Source, doc.ContentHash, nowText(doc.CreatedAt), nowText(doc.UpdatedAt))
		if err != nil {
			return err
		}
		doc.ID, err = result.LastInsertId()
		return err
	}
	_, err := s.DB.ExecContext(ctx, `UPDATE knowledge_documents SET title=?,kind=?,content=?,source=?,content_hash=?,updated_at=? WHERE id=? AND base_id=?`, doc.Title, doc.Kind, doc.Content, doc.Source, doc.ContentHash, nowText(doc.UpdatedAt), doc.ID, doc.BaseID)
	return err
}

func (s *Store) ListKnowledgeBasesDocuments(ctx context.Context, accountID string, baseID int64) ([]groupmgr.KnowledgeDocument, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash,d.created_at,d.updated_at FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id WHERE b.account_id=? AND (?=0 OR d.base_id=?) ORDER BY d.title,d.id`, accountID, baseID, baseID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanKnowledgeDocuments(rows)
}

func (s *Store) BindKnowledgeBaseGroups(ctx context.Context, accountID string, baseID int64, groupIDs []int64) error {
	if baseID == 0 {
		return errors.New("请选择知识库")
	}
	var baseAccount string
	if err := s.DB.QueryRowContext(ctx, `SELECT account_id FROM knowledge_bases WHERE id=?`, baseID).Scan(&baseAccount); err != nil {
		return err
	}
	if baseAccount != accountID {
		return errors.New("知识库账号不匹配")
	}
	now := nowText(time.Now())
	for _, groupID := range groupIDs {
		if _, err := s.DB.ExecContext(ctx, `INSERT INTO knowledge_base_groups(base_id,account_id,group_id,enabled,created_at) VALUES(?,?,?,?,?) ON CONFLICT(base_id,account_id,group_id) DO UPDATE SET enabled=1`, baseID, accountID, groupID, 1, now); err != nil {
			return err
		}
	}
	return nil
}

func (s *Store) UnbindKnowledgeBaseGroups(ctx context.Context, accountID string, baseID int64, groupIDs []int64) error {
	for _, groupID := range groupIDs {
		if _, err := s.DB.ExecContext(ctx, `DELETE FROM knowledge_base_groups WHERE base_id=? AND account_id=? AND group_id=?`, baseID, accountID, groupID); err != nil {
			return err
		}
	}
	return nil
}

func (s *Store) ListKnowledgeBindings(ctx context.Context, accountID string, baseID int64) ([]groupmgr.KnowledgeBinding, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT base_id,account_id,group_id,enabled,created_at FROM knowledge_base_groups WHERE account_id=? AND (?=0 OR base_id=?) ORDER BY group_id`, accountID, baseID, baseID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.KnowledgeBinding
	for rows.Next() {
		var binding groupmgr.KnowledgeBinding
		var enabled int64
		var created sql.NullString
		if err := rows.Scan(&binding.BaseID, &binding.AccountID, &binding.GroupID, &enabled, &created); err != nil {
			return nil, err
		}
		binding.Enabled, binding.CreatedAt = enabled != 0, parseTime(created)
		out = append(out, binding)
	}
	return out, rows.Err()
}

func (s *Store) ListKnowledgeForGroup(ctx context.Context, accountID string, groupID int64) ([]groupmgr.KnowledgeDocument, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash,d.created_at,d.updated_at FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id JOIN knowledge_base_groups g ON g.base_id=b.id AND g.account_id=b.account_id WHERE b.account_id=? AND g.group_id=? AND b.enabled=1 AND g.enabled=1 ORDER BY b.name,d.title,d.id`, accountID, groupID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanKnowledgeDocuments(rows)
}

func (s *Store) ListKnowledgeForGroups(ctx context.Context, accountID string, groupIDs []int64) ([]groupmgr.KnowledgeDocument, error) {
	if len(groupIDs) == 0 {
		return nil, nil
	}
	placeholders := strings.TrimRight(strings.Repeat("?,", len(groupIDs)), ",")
	args := make([]any, 0, len(groupIDs)+1)
	args = append(args, accountID)
	for _, groupID := range groupIDs {
		args = append(args, groupID)
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT DISTINCT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash,d.created_at,d.updated_at FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id JOIN knowledge_base_groups g ON g.base_id=b.id AND g.account_id=b.account_id WHERE b.account_id=? AND g.group_id IN (`+placeholders+`) AND b.enabled=1 AND g.enabled=1 ORDER BY b.name,d.title,d.id`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanKnowledgeDocuments(rows)
}

func (s *Store) scanKnowledgeBase(row interface{ Scan(...any) error }, base *groupmgr.KnowledgeBase) error {
	var enabled, builtIn, readOnly int64
	var created, updated sql.NullString
	if err := row.Scan(&base.ID, &base.AccountID, &base.Name, &base.Description, &enabled, &builtIn, &readOnly, &created, &updated); err != nil {
		return err
	}
	base.Enabled, base.BuiltIn, base.ReadOnly = enabled != 0, builtIn != 0, readOnly != 0
	base.CreatedAt, base.UpdatedAt = parseTime(created), parseTime(updated)
	return nil
}

func scanKnowledgeDocuments(rows *sql.Rows) ([]groupmgr.KnowledgeDocument, error) {
	var out []groupmgr.KnowledgeDocument
	for rows.Next() {
		var doc groupmgr.KnowledgeDocument
		var created, updated sql.NullString
		if err := rows.Scan(&doc.ID, &doc.BaseID, &doc.BaseName, &doc.Title, &doc.Kind, &doc.Content, &doc.Source, &doc.ContentHash, &created, &updated); err != nil {
			return nil, err
		}
		doc.CreatedAt, doc.UpdatedAt = parseTime(created), parseTime(updated)
		out = append(out, doc)
	}
	return out, rows.Err()
}

func (s *Store) MigrateLegacyKnowledge(ctx context.Context, accountID string) error {
	if strings.TrimSpace(accountID) == "" {
		return nil
	}
	if _, err := s.EnsureDefaultKnowledgeBase(ctx, accountID); err != nil {
		return err
	}
	var count int
	if err := s.DB.QueryRowContext(ctx, `SELECT COUNT(*) FROM knowledge_bases WHERE account_id=? AND name LIKE '旧知识库 · %'`, accountID).Scan(&count); err != nil {
		return err
	}
	if count > 0 {
		return nil
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,group_id,kind,title,content,source,created_at,updated_at FROM knowledge WHERE group_id=0 OR group_id IN (SELECT group_id FROM groups WHERE account_id=?) ORDER BY id`, accountID)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var old groupmgr.Knowledge
		var created, updated sql.NullString
		if err := rows.Scan(&old.ID, &old.GroupID, &old.Kind, &old.Title, &old.Content, &old.Source, &created, &updated); err != nil {
			return err
		}
		base := groupmgr.KnowledgeBase{AccountID: accountID, Name: fmt.Sprintf("旧知识库 · %s", old.Title), Description: "从旧版知识内容迁移，可复制后编辑。", Enabled: true, CreatedAt: parseTime(created), UpdatedAt: parseTime(updated)}
		if err := s.CreateKnowledgeBase(ctx, &base); err != nil {
			return err
		}
		doc := groupmgr.KnowledgeDocument{BaseID: base.ID, Title: old.Title, Kind: old.Kind, Content: old.Content, Source: old.Source, CreatedAt: parseTime(created), UpdatedAt: parseTime(updated)}
		if err := s.UpsertKnowledgeDocument(ctx, &doc); err != nil {
			return err
		}
		if old.GroupID == 0 {
			groups, err := s.ListGroups(ctx, accountID, false)
			if err != nil {
				return err
			}
			ids := make([]int64, 0, len(groups))
			for _, group := range groups {
				ids = append(ids, group.GroupID)
			}
			if err := s.BindKnowledgeBaseGroups(ctx, accountID, base.ID, ids); err != nil {
				return err
			}
		} else if err := s.BindKnowledgeBaseGroups(ctx, accountID, base.ID, []int64{old.GroupID}); err != nil {
			return err
		}
	}
	return rows.Err()
}
