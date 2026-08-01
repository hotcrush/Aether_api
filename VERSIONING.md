# Aether 版本规范

## 版本号格式

遵循 [Semantic Versioning 2.0.0](https://semver.org/lang/zh-CN/)，格式为：

```
MAJOR.MINOR.PATCH[-PRERELEASE]
```

| 段 | 递增时机 |
|---|---|
| MAJOR | 不兼容的架构变更（如数据库格式破坏性迁移、代理协议不兼容） |
| MINOR | 向后兼容的功能新增（如新增市场监控、WebView 工作区） |
| PATCH | 向后兼容的缺陷修复和小改进 |
| PRERELEASE | 可选预发布标识，见下方生命周期 |

## 发布生命周期

```
0.1.0-alpha.1 → 0.1.0-alpha.2 → ... → 0.1.0-beta.1 → ... → 0.1.0-rc.1 → ... → 0.1.0
```

| 阶段 | 含义 | 进入条件 |
|---|---|---|
| `alpha.N` | 内部测试，功能不完整，可能有破坏性变更 | 功能开发中，仅开发者自用 |
| `beta.N` | 功能基本完整，公开测试，仍可能有破坏性变更 | 核心功能已就绪，邀请外部测试 |
| `rc.N` | 候选发布，除非发现阻塞缺陷否则不再变更 | 所有已知问题已修复，准备正式发布 |
| 正式版 | 无后缀 | rc 阶段无阻塞缺陷 |

当前版本：`0.1.0-alpha.1`

## 版本同步点

发版时必须同步修改以下 **3 个文件** 中的版本号：

| 文件 | 字段 |
|---|---|
| `package.json` | `"version"` |
| `src-tauri/Cargo.toml` | `[package] version` |
| `src-tauri/tauri.conf.json` | `"version"` |

三处版本号必须完全一致。

## 构建元数据

`build.rs` 在编译时自动注入以下环境变量，无需手动维护：

| 环境变量 | 来源 | 示例 |
|---|---|---|
| `AETHER_GIT_COMMIT` | `git rev-parse --short HEAD` | `6331632` |
| `AETHER_BUILD_TIME` | 编译时刻 UTC RFC 3339 | `2026-08-01T12:34:56Z` |

前端通过 `get_app_version` 命令获取完整版本信息（版本号、commit、构建时间、debug/release、Tauri 版本），显示在标题栏品牌区域。

## Git 提交规范

提交信息格式：

```
<type>: <简要描述>

[可选正文]

[可选脚注]
```

### type 取值

| type | 用途 |
|---|---|
| `feat` | 新功能 |
| `fix` | 缺陷修复 |
| `refactor` | 重构（不改变外部行为） |
| `perf` | 性能优化 |
| `style` | 格式调整（不影响逻辑） |
| `docs` | 文档变更 |
| `chore` | 构建、依赖、配置等杂项 |
| `test` | 测试相关 |

### 规则

- 描述用祈使句，首字母小写，不加句号：`feat: add market monitor`
- 涉及模块时可加作用域：`feat(market): add shop circuit breaker`
- 破坏性变更在脚注中标注：`BREAKING CHANGE: ...`
- 关联 Issue：`Closes #12`

## 发版流程

1. 确认所有变更已提交，工作区干净。
2. 同步更新 3 个文件的版本号。
3. 更新 `CHANGELOG.md`，将 `[Unreleased]` 下的条目移到新版本标题下。
4. 提交：`chore: release vX.Y.Z`
5. 打标签：`git tag vX.Y.Z`
6. 构建：`pnpm tauri build`
7. 推送：`git push && git push --tags`

## 时间格式约定

项目中所有时间统一使用 UTC，格式遵循 RFC 3339：

| 场景 | 格式 | 示例 |
|---|---|---|
| SQLite 存储 | `strftime('%Y-%m-%dT%H:%M:%SZ', 'now')` | `2026-08-01T12:34:56Z` |
| 请求日志（含毫秒） | `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')` | `2026-08-01T12:34:56.789Z` |
| Rust 代码 | `Utc::now().to_rfc3339()` | `2026-08-01T12:34:56.789+00:00` |
| Unix 时间戳 | 仅 `expires_at` 使用 i64 秒 | `1754049296` |
| 前端显示 | `src/lib/time.ts` 统一格式化 | 见模块注释 |
