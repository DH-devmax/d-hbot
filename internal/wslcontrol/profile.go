package wslcontrol

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

const defaultProfileID = "dh-primary"

const legacyPartitionCode = "persistFlag=`persist:${Date.now()}`"

const fixedPartitionCode = "persistFlag=process.env.DH_WSL_PARTITION?`persist:${process.env.DH_WSL_PARTITION}`:`persist:${Date.now()}`"

func patchProfileScript(source []byte) ([]byte, bool, error) {
	text := string(source)
	if strings.Contains(text, fixedPartitionCode) {
		return append([]byte(nil), source...), false, nil
	}
	if !strings.Contains(text, legacyPartitionCode) {
		return nil, false, errors.New("旺商聊启动脚本版本不匹配")
	}
	return []byte(strings.Replace(text, legacyPartitionCode, fixedPartitionCode, 1)), true, nil
}

func matchingProfileBackup(backupDir string, patched []byte) string {
	entries, err := os.ReadDir(backupDir)
	if err != nil {
		return ""
	}
	type candidate struct {
		path     string
		modified time.Time
	}
	matches := make([]candidate, 0, len(entries))
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasPrefix(name, "wangshangliao-") || !strings.HasSuffix(name, ".index.js") {
			continue
		}
		path := filepath.Join(backupDir, name)
		original, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		candidateBytes, changed, err := patchProfileScript(original)
		if err != nil || !changed || !bytes.Equal(candidateBytes, patched) {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		matches = append(matches, candidate{path: path, modified: info.ModTime()})
	}
	if len(matches) == 0 {
		return ""
	}
	sort.SliceStable(matches, func(i, j int) bool { return matches[i].modified.After(matches[j].modified) })
	return matches[0].path
}

func restoreProfileScript(scriptPath, backupPath string) error {
	if scriptPath == "" || backupPath == "" {
		return errors.New("旺商聊原文件备份信息不完整")
	}
	original, err := os.ReadFile(backupPath)
	if err != nil {
		return err
	}
	if _, changed, err := patchProfileScript(original); err != nil || !changed {
		return errors.New("旺商聊原文件备份校验失败")
	}
	tmp := scriptPath + ".dh-restore"
	if err := os.WriteFile(tmp, original, 0o600); err != nil {
		return err
	}
	if err := os.Remove(scriptPath); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return os.Rename(tmp, scriptPath)
}

func migrateLatestPartition(root, profileID string) bool {
	partitions := filepath.Join(root, "Partitions")
	entries, err := os.ReadDir(partitions)
	if err != nil {
		return false
	}
	if profileID == "" {
		profileID = defaultProfileID
	}
	target := filepath.Join(partitions, profileID)
	if _, err := os.Stat(target); err == nil {
		return false
	}
	type candidate struct {
		name     string
		modified time.Time
	}
	candidates := make([]candidate, 0, len(entries))
	for _, entry := range entries {
		if !entry.IsDir() || entry.Name() == profileID {
			continue
		}
		if _, err := strconv.ParseInt(entry.Name(), 10, 64); err != nil {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		candidates = append(candidates, candidate{name: entry.Name(), modified: info.ModTime()})
	}
	if len(candidates) == 0 {
		return false
	}
	sort.SliceStable(candidates, func(i, j int) bool {
		if candidates[i].modified.Equal(candidates[j].modified) {
			return candidates[i].name > candidates[j].name
		}
		return candidates[i].modified.After(candidates[j].modified)
	})
	return os.Rename(filepath.Join(partitions, candidates[0].name), target) == nil
}
