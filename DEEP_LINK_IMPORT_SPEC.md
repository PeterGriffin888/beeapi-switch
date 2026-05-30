# BeeAPI Switch 超链导入规范

用于网页按钮或第三方 AI 工具生成兼容链接。

## 协议

- `beeapi-switch://`
- `beeapi://`

## 推荐格式

```text
beeapi-switch://import?key=sk-xxx&name=key-1
```

也支持：

```text
beeapi://import?key=sk-xxx&name=key-1
```

## 支持参数

### 密钥

任一参数都表示 API Key：

- `key`
- `api_key`
- `apikey`
- `token`
- `secret`

### 密钥名称

任一参数都表示密钥名称 / 标签：

- `name`
- `label`
- `key_name`
- `keyName`
- `title`

### 其它可用参数

- `model`：可选，导入后预填模型
- `pool`
- `pool_enabled`

`pool_enabled` 支持：

- `1 / 0`
- `true / false`
- `yes / no`
- `on / off`

### 会被忽略的参数

以下参数当前会忽略，不会影响配置：

- `base`
- `base_url`
- `api`
- `api_url`
- `api_address`
- `upstream`

## 支持形式

### 1) 单密钥

```text
beeapi-switch://import?key=sk-xxx&name=主密钥
```

### 2) 带模型

```text
beeapi-switch://import?key=sk-xxx&name=主密钥&model=gpt-4.1-mini
```

### 3) 多密钥

```text
beeapi-switch://import?keys=sk-1,sk-2,sk-3&name=导入密钥
```

也支持重复写多个 `key=`：

```text
beeapi-switch://import?key=sk-1&key=sk-2&key=sk-3
```

### 4) 紧凑写法

```text
beeapi-switch://sk-xxx?name=key-1
beeapi://sk-xxx
```

## 行为

- 重复密钥会跳过
- 明显无效的密钥会忽略
- 导入后会自动刷新 UI
- 如果当前没有主密钥，第一把导入的密钥会自动设为主密钥
- 不会通过链接修改上游地址；默认固定使用 `https://bate.beeapi.ai/`

## 单 exe 支持

现在也可以直接分发单个 `beeapi-switch.exe`：

- 需要先在应用里点一次“注册一键导入服务”
- 注册会写入当前用户的协议注册
- 之后点击上述链接会直接拉起这个 exe
- 如果 exe 被移动/改名，再点一次注册即可刷新协议指向

## 兼容建议

如果你要在别的 AI / 工具里兼容：

1. 优先生成 `beeapi-switch://import?...`
2. 至少支持 `key` 和 `name`
3. `base_url` 可以继续出现在链接里，但 BeeAPI Switch 当前会忽略
4. 不要依赖链接修改上游地址
