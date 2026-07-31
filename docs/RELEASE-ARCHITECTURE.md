# DH BOT 个人发行架构

## 归属

- `DH-devmax/d-hbot`：私有源码仓库，Actions 停用。
- `DH-devmax/d-hbot-releases`：可选下载说明或历史索引，Actions 停用。
- Windows 开发机：唯一生产构建、扫描和实机验收环境。
- 管理员云盘：个人用户下载 portable ZIP、手册和 SHA-256 清单的位置。

## 发布数据流

```text
固定源码 commit
  -> 本地自动测试
  -> Windows --no-default-features 生产构建
  -> NSIS 与 portable 深度隔离扫描
  -> Windows 真实桌面验收
  -> 生成 SHA256SUMS.txt
  -> 上传管理员云盘
  -> 下载后复核 SHA-256
```

当前个人发行采用未签名产物。Windows 可能显示“未知发布者”或 SmartScreen 提示；云盘页面需要同时公开 `SHA256SUMS.txt`，便于用户核对文件完整性。

## 产物边界

主发行文件：

```text
DH-BOT-VERSION-windows-x64-portable.zip
SHA256SUMS.txt
DH-Manual-ZH.pdf
```

可选文件：

```text
DH-BOT-VERSION-windows-x64-setup.exe
DH-Manual-ZH.md
DH-BOT-Default-Rules.json
```

云盘中不得出现 Fixture、开发 EXE、`9233/51300`、测试数据库、源码、PDB、Source Map、PFX、私钥、密码、Token、Cookie、原始契约或真实群数据。

## 发布记录

每次发行应在本地记录：

- 应用版本和完整源码 commit SHA。
- 构建日期与 Windows 版本。
- portable ZIP、主 EXE 和安装包 SHA-256。
- 生产隔离扫描结果。
- Windows 实机验收报告路径。
- 已上传云盘文件清单。

已有版本不覆盖；修复后增加版本号并生成新的校验清单。云盘链接失效时只重新上传字节完全一致的文件，否则视为新构建。

## 用户排障资料

生产包保留“调试 → 生成诊断包”入口，但诊断包不是发行产物，也不会自动上传。使用者复现问题后可生成本地 ZIP，再把完整 ZIP、发生时间、操作步骤和截图发给维护人员。包内只含脱敏状态、匿名化审计、近期脱敏日志、`manifest.json` 与 `SHA256SUMS.txt`；不含数据库、旺商聊登录分区、密钥、原始消息或 Fixture。接收后先用包内校验文件核对完整性，再按会话 ID、序号和 UTC 时间重建故障顺序。
