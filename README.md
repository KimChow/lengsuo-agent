# lengsuo-agent

Lengsuo 智能体平台：会话与任务运行时、工具执行、Skills、MCP、记忆、沙箱与桌面壳（GPUI）。

## 本仓范围

| 层 | crate |
|---|---|
| 运行时核心 | `agent-core`（会话/任务模型、事件日志、工具注册表、权限门、模式系统） |
| 本地网关 | `agent-gateway`（`127.0.0.1` HTTP+WS、token 鉴权、`域:动作` RPC、事件订阅） |
| 协议接入 | `agent-acp`（stdio 传输 + 请求路由）、`agent-mcp` |
| 能力层 | `agent-tools` / `agent-tools-exec`（文件、Bash、WebFetch）、`agent-pty`（终端） |
| 扩展 | `agent-skills`（技能发现/解析/优先级）、`agent-prompts`、`agent-tasks` |
| 记忆 | `agent-memory`（三层记忆 + 隔离） |
| 安全 | `agent-sandbox`（Linux bwrap+seccomp / Windows 受限令牌）、`agent-execpolicy`、`policy-engine`、`credential-ref` |
| 桌面壳 | `lengsuo-shell`（GPUI 网关客户端 + 视图状态） |

## 与其他仓的边界

- **网络工具与指纹浏览器**在 `lengsuo-network`（原 `lengsuo-client`；八协议、TUN、环境隔离、浏览器组件）。
- **服务端权威逻辑**（授权/计费/节点池）在独立的私有后端仓。
- 跨模块接口以 `docs/contracts/` 为准。

## 门禁

```text
python scripts/check_line_limit.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
```

单文件 700 行硬上限 / 400 行软目标。生产网络代码禁用 `unsafe`。

## 许可

源码可查看，不等于开源授权。见 `LICENSE` 与 `LICENSES/`。
