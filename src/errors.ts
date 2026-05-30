/**
 * Translate common API error messages and codes into readable Chinese.
 */

const ERROR_MAP: Record<string, string> = {
  // BeeAPI / sub2api error codes
  API_KEY_DISABLED: "密钥已被禁用，请到 beeapi.ai 后台检查",
  API_KEY_INVALID: "密钥无效，请检查是否输入正确",
  API_KEY_EXPIRED: "密钥已过期，请重新生成",
  INSUFFICIENT_BALANCE: "余额不足，请充值后再试",
  INSUFFICIENT_QUOTA: "配额不足，请充值或等待重置",
  RATE_LIMIT_EXCEEDED: "请求过于频繁，请稍后再试",
  MODEL_NOT_FOUND: "模型不存在，请检查模型名称",
  MODEL_NOT_AVAILABLE: "该模型暂不可用",
  UPSTREAM_ERROR: "上游服务异常，请稍后再试",
  ACCOUNT_SUSPENDED: "账户已被暂停",
  PERMISSION_DENIED: "权限不足",
  INVALID_REQUEST: "请求格式错误",
  CONTEXT_LENGTH_EXCEEDED: "上下文长度超限",
  CONTENT_POLICY_VIOLATION: "内容违反使用政策",
  SERVER_ERROR: "服务器内部错误",
  SERVICE_UNAVAILABLE: "服务暂时不可用",
  GATEWAY_TIMEOUT: "网关超时",
  // OpenAI-style
  invalid_api_key: "密钥无效",
  insufficient_quota: "配额不足",
  rate_limit_exceeded: "请求频率超限",
  model_not_found: "模型不存在",
};

const STATUS_MAP: Record<number, string> = {
  400: "请求格式错误",
  401: "认证失败（密钥无效或已禁用）",
  402: "余额不足",
  403: "访问被拒绝",
  404: "接口不存在，请检查地址",
  408: "请求超时",
  429: "请求过于频繁",
  500: "上游服务器错误",
  502: "网关错误",
  503: "服务暂时不可用",
  504: "网关超时",
};

const NETWORK_PATTERNS: [RegExp, string][] = [
  [/timeout|timed out/i, "连接超时，请检查网络"],
  [/connect|connection refused/i, "无法连接服务器，请检查网络或地址"],
  [/dns|resolve/i, "域名解析失败，请检查网络"],
  [/certificate|ssl|tls/i, "SSL 证书错误"],
  [/network|fetch/i, "网络错误，请检查连接"],
];

export function humanizeError(raw: string): string {
  if (!raw) return "未知错误";

  // Try to extract JSON error code from the message
  const codeMatch = raw.match(/"code"\s*:\s*"([^"]+)"/);
  if (codeMatch) {
    const code = codeMatch[1];
    if (ERROR_MAP[code]) {
      return ERROR_MAP[code];
    }
  }

  // Try to extract "message" field
  const msgMatch = raw.match(/"message"\s*:\s*"([^"]+)"/);
  if (msgMatch) {
    const msg = msgMatch[1];
    // Check if the message itself is a known code
    if (ERROR_MAP[msg]) return ERROR_MAP[msg];
    // Check if it contains a known code
    for (const [code, translation] of Object.entries(ERROR_MAP)) {
      if (msg.includes(code)) return translation;
    }
  }

  // Try HTTP status code pattern: "返回 4xx" or "returned 4xx"
  const statusMatch = raw.match(/返回\s*(\d{3})|returned?\s*(\d{3})|status[:\s]*(\d{3})/i);
  if (statusMatch) {
    const status = parseInt(statusMatch[1] || statusMatch[2] || statusMatch[3]);
    if (STATUS_MAP[status]) {
      // If we also have a code, combine them
      const combined = codeMatch ? `${STATUS_MAP[status]}（${codeMatch[1]}）` : STATUS_MAP[status];
      return combined;
    }
  }

  // Check for known error codes anywhere in the string
  for (const [code, translation] of Object.entries(ERROR_MAP)) {
    if (raw.includes(code)) {
      return translation;
    }
  }

  // Network error patterns
  for (const [pattern, translation] of NETWORK_PATTERNS) {
    if (pattern.test(raw)) {
      return translation;
    }
  }

  // If it's short enough and already in Chinese, return as-is
  if (raw.length < 100) return raw;

  // Truncate long errors
  return raw.slice(0, 120) + "…";
}
