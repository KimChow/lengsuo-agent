# x-token-usage · token 记账入口（批 2 · 接线）状态

本卡只做**记账入口**：把一次模型调用的 token 用量落成事件日志里的一条
`model.usage` 事件。不做预算上限、超额阻断、金额换算、跨会话累计。

## 1. 记账链路

```
Provider::next_steps_with_context / with_options
        │  返回 Vec<ProviderStep>（本次调用已成功）
        ▼
Provider::last_usage() -> Option<TokenUsage>
        │  None ⇒ 不记账（不落全 0 假记录）
        │  Some ⇒
        ▼
Runtime::step_with_write_ledger（runtime/turns.rs）
        │  Runtime::record_provider_usage（None 直接返回）
        │  events.push(...)
        ▼
runtime::usage_log::record_usage(&dyn EventLog, session_id, usage)
        │  log.append(session_id, "model.usage", payload)
        ▼
EventLog（JSONL）── replay / session:replay 可查
```

落点：`step_with_write_ledger` 在 provider 调用返回步骤序列之后、`consume_step`
消费/结算之前。`surrender_step` 也走同一条原语，故缴械步同样记账。

## 2. `model.usage` payload 键表（5 个键全部必填）

| 键 | 来源 |
| --- | --- |
| `prompt_tokens` | `TokenUsage::prompt_tokens` |
| `completion_tokens` | `TokenUsage::completion_tokens` |
| `total_tokens` | `TokenUsage::total_tokens` |
| `cached_tokens` | `TokenUsage::cached_tokens` |
| `billed_tokens` | `TokenUsage::billed_tokens()`（复用，不另算） |

不加 `model` / `provider` / `cost` 键；事件 kind 固定为 `model.usage`。

## 3. `billed_tokens` 的复用点

`crates/agent-core/src/provider.rs:39` 的 `TokenUsage::billed_tokens()`：

```rust
self.prompt_tokens.saturating_sub(self.cached_tokens) + self.completion_tokens
```

`runtime/usage_log.rs` 只调用该方法，仓库内没有第二份计费公式；
`contracts-check/tests/provider_usage_surface.rs` 以源码文本断言守住这处口径。

## 4. `Provider` trait 加方法的影响面

新增（带默认实现）：

```rust
fn last_usage(&self) -> Option<TokenUsage> { None }
```

- 默认实现 ⇒ 既有实现方**零改动、不破编译**：`EchoProvider`、
  `UnimplementedProvider`、网关/运行时测试里的各种桩 provider、`loop` 侧实现都不必改。
- 真实适配器 `OpenAiProvider`（`agent-openai` crate）覆写它，只读自己已有的
  usage 槽位；同名固有方法仍保留（公开 API 不变）。
- 这是**公开 API 增量**（trait 多一个带默认实现的方法），已在
  `contracts-check` 登记。

## 5. `contracts-check` 登记与护栏自证

`crates/contracts-check/tests/provider_usage_surface.rs`（源码文本扫描，
`include_str!` 编译期绑定）：

1. `Provider` trait 里有 `fn last_usage(&self) -> Option<TokenUsage>`；
2. `TokenUsage` 保留 4 个 OpenAI 规范字段名；
3. `billed_tokens()` 保留「输入 − 缓存命中 + 输出」口径；
4. `OpenAiProvider` 的 `impl Provider` 里实现了 `last_usage`。

口径是**只守关系，不锁方法/字段数量**。所有断言走同一个 `assert_contains`；
护栏自证 `guard_detects_source_drift` 用合成漂移样本调它，`catch_unwind`
必须捕获到 panic（捕获不到即本卡无效）。

## 6. 测试清单（`crates/agent-core/tests/usage_log.rs`）

1. `record_usage_writes_exactly_one_model_usage_event`
2. `payload_carries_all_five_keys_matching_the_usage`
3. `billed_tokens_reuses_the_token_usage_caliber`
4. `missing_usage_writes_no_event_and_no_zero_record`
5. `usage_event_survives_replay`
6. `runtime_turn_records_usage_from_provider`（接线：`send_text` 一回合后日志里
   恰有一条 `model.usage`）

## 7. 未做项 / 已知边界

- **预算上限、超额阻断、计费金额换算**：不在本卡。
- **跨会话累计**：不在本卡（需要账本持久化）。
- **usage 缺失**（`last_usage() == None`）：不写事件，不落全 0 假记录；
  是否上报由调用方决定，本卡不引入任何全局计数器/全局状态。
- **OpenAI 适配器的槽位语义**：`call()` 只在响应带 `usage` 时覆盖槽位，
  因此「上一次带 usage、下一次不带」时 `last_usage()` 仍会看到上一次的值。
  本卡按定死口径返回该值；要让「无 usage」严格按调用清空，需在每次 `call`
  开始时清槽（`agent-openai` 的 `call` 不在本卡可改范围，留待后续卡）。
- **模块路径偏差**：卡面写的是 `crates/agent-core/src/runtime/mod.rs`，实际仓内
  模块根是 `crates/agent-core/src/runtime.rs`（同目录 `runtime/` 只放子模块）；
  `pub mod usage_log;` 因此加在 `runtime.rs`。
