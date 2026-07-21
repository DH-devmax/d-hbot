package migration

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
)

type LegacyConnection struct {
	Endpoint       string
	DefaultGroupID int64
}

type legacyState struct {
	Settings struct {
		Endpoint       string `json:"endpoint"`
		DefaultGroupID int64  `json:"defaultGroupId"`
	} `json:"settings"`
}

func ReadLegacy(path string) (LegacyConnection, bool, error) {
	raw, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return LegacyConnection{}, false, nil
	}
	if err != nil {
		return LegacyConnection{}, false, fmt.Errorf("读取旧版设置失败：%w", err)
	}
	var state legacyState
	if err := json.Unmarshal(raw, &state); err != nil {
		return LegacyConnection{}, true, fmt.Errorf("解析旧版设置失败：%w", err)
	}
	return LegacyConnection{Endpoint: state.Settings.Endpoint, DefaultGroupID: state.Settings.DefaultGroupID}, true, nil
}

func RemoveLegacy(path string) error {
	if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("清理旧版设置失败：%w", err)
	}
	if err := os.Remove(path + ".tmp"); err != nil && !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("清理旧版临时设置失败：%w", err)
	}
	return nil
}
