package secrets

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

type Store struct{ Path string }

func DefaultPath() string {
	base, err := os.UserConfigDir()
	if err != nil || base == "" {
		base = "."
	}
	return filepath.Join(base, "DH", "secrets.dat")
}

func (store Store) Load() (map[string]string, error) {
	raw, err := os.ReadFile(store.Path)
	if os.IsNotExist(err) {
		return map[string]string{}, nil
	}
	if err != nil {
		return nil, fmt.Errorf("读取密钥配置失败：%w", err)
	}
	plain, err := unprotect(raw)
	if err != nil {
		return nil, fmt.Errorf("解密密钥配置失败：%w", err)
	}
	values := make(map[string]string)
	if err := json.Unmarshal(plain, &values); err != nil {
		return nil, fmt.Errorf("解析密钥配置失败：%w", err)
	}
	return values, nil
}

func (store Store) Save(values map[string]string) error {
	raw, err := json.Marshal(values)
	if err != nil {
		return fmt.Errorf("编码密钥配置失败：%w", err)
	}
	protected, err := protect(raw)
	if err != nil {
		return fmt.Errorf("加密密钥配置失败：%w", err)
	}
	if err := os.MkdirAll(filepath.Dir(store.Path), 0o700); err != nil {
		return fmt.Errorf("创建密钥目录失败：%w", err)
	}
	temporary := store.Path + ".tmp"
	if err := os.WriteFile(temporary, protected, 0o600); err != nil {
		return fmt.Errorf("写入密钥配置失败：%w", err)
	}
	if err := os.Rename(temporary, store.Path); err != nil {
		return fmt.Errorf("更新密钥配置失败：%w", err)
	}
	return nil
}
