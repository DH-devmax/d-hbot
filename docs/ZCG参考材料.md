# ZCG 参考材料说明

本工作区保留两类历史材料，仅用于协议、数据字段和业务流程对照：

- `../zcg26.6.28`：原始 ZCG 运行样本及本地数据库。包含下注、算账、开奖、结算等旧业务，DH BOT 3.0 不会读取、打包或执行这些功能。

DH BOT 只内置 ZCG 已验证的群列表、成员、禁言、解禁、改名、移出、撤回、全群禁言和消息编码协议基线，不依赖 ZCG 程序、`xplugin.exe` 或 `51234` 端口。
- `../recovered-src`：从样本整理出的协议、字符串、HTTP 地址和运行记录。它用于确认旺商聊群管理接口、NIM 字段以及预测数据适配器的来源。

DH BOT 的预测层只把已确认的数据适配器编译进 EXE，并在回复中输出整理后的期号、结果、更新时间、统计趋势、候选方向和参考度。原始接口地址、认证信息、上游响应结构和 ZCG 财务数据不会进入发布包或群消息。

ZCG 中确认的是开奖与历史数据契约，不是直接输出结论的“预测接口”。当前已整理 `pcdd`、`jnd`、`btc28`、`bj28`、`tx28` 五个请求名称，以及 `issue/kjcodes/time`、`expect/opencode/opentime`、`preDrawIssue/preDrawCode/preDrawTime`、`full_expect/open_code/open_time` 四类响应字段。动态配置中的旧主机已经失效，`gsdatas` 与 `mbf52` 备用服务仍要求 Token。

DH BOT 不复用 ZCG 登录 Token。开发环境可通过临时 `DH_PREDICTION_TOKEN` 验证脱敏契约；没有独立、可持续凭据时，生产预测应用保持停用。旧网页抓取和通用正则适配器已经移除。

完整校验和见 `ZCG-REFERENCE-SHA256.txt`。发布包只携带 `DH-BOT-Default-Rules.json`，该文件是 DH BOT 群管规则模板，只保留经验上兼容的规则行为。
