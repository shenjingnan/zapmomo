/**
 * zapmomo-bridge —— dsh（deepseek-harness）→ ZapMomo 的任务事件桥。
 *
 * 职责：监听 dsh 的 agent/session 事件，把「任务开始 / 结束 / 失败 / 中断」翻译成
 * 语义化事件 POST 到 ZapMomo 的 loopback HTTP 桥（`POST /dsh/events`），供桌宠
 * 在任务状态翻转瞬间给出毫秒级反馈（模板台词等）。
 *
 * ─────────────────────────────────────────────────────────────────────────────
 * 挂载方式（已对照 dsh 源码确认）
 * ─────────────────────────────────────────────────────────────────────────────
 * 本包声明了 `dsh.bundle.patch`（见 package.json + cordis.patch.yml），因此用一条
 * 命令即可完成「安装 + 注册 bundle + 应用 patch」：
 *
 *   dsh plugin --profile web add ~/.dsh/plugins/zapmomo-bridge
 *
 * 命令底层是 `dsh plugin` 在 profile 目录里执行 `pnpm add <路径>`，再把声明了
 * `dsh.bundle.patch` 的依赖加入 `~/.dsh/profiles/web/package.json` 的
 * `dsh.profile.bundles`（见 dsh 仓库 apps/cli/src/plugin.ts 的 reconcilePlugins；
 * patch 行格式见 packages/bundle/base/cordis.patch.yml 的 `insert` 条目）。
 *
 * 开发期希望改动即时生效（pnpm `file:` 会拷贝目录），可改用 link 依赖：
 *
 *   dsh plugin --profile web add link:$HOME/.dsh/plugins/zapmomo-bridge
 *
 * 若不用 bundle 机制，也可以手动挂载（两 步，缺一不可）：
 *   1. `dsh plugin --profile web add ~/.dsh/plugins/zapmomo-bridge`  （安装为依赖）
 *   2. 在 ~/.dsh/profiles/web/cordis.patch.yml 追加：
 *      - insert:
 *          - id: zapmomo-bridge
 *            name: '@zapmomo-ai/dsh-plugin'
 * 注意：仅 `dsh plugin ... add` 而不在 patch 里插入，插件不会被加载；
 * 仅插 patch 而不安装依赖，loader 无法 import 到该包。
 *
 * ─────────────────────────────────────────────────────────────────────────────
 * 字段路径核对（2026-08-21，对照 dsh 源码；与最初探索结论的差异见下方清单）
 * ─────────────────────────────────────────────────────────────────────────────
 * 1. `session/event` 处理器是 **两个参数** `(session, event)`，sessionId 取自第一参
 *    `session.id`，事件体是 `{ type, seq, time, data }`（见 dsh cookbook
 *    extension-cookbook.md 的 UI 插件示例；packages/core/session/src/types.ts 的
 *    SessionEvent 定义）。最初假定 `(ev)` 单参 + `ev.sessionId` —— 已修正。
 * 2. `user/message` 的 `data` 是一整个 UserMessage，`data.content` 是 **ContentBlock[]**
 *    数组，用户文本在 `type === 'text'` 块的 `.text` 字段；不是字符串
 *    `data.content`（最初假定 `data.content ?? data.text` 取字符串 —— 已修正）。
 * 3. `turn/end` 的 `data = { turn, reason }`，`reason.kind` 取值
 *    completed/aborted/blocked/error/max-tokens/interrupted（types.ts 的
 *    TurnEndReasonMap）。错误详情在 `data.reason.error: LlmFailure`（`.message` /
 *    `.code`），**没有** `detail` 字段（最初假定 `data.reason.detail` —— 已修正）。
 * 4. `agent/status` 载荷是 `{ agent, status }`：`agent` 由 fused dispatch 注入，
 *    sessionId = `agent.id`，`running` 由 `status === 'running'` 派生（runtime-types.ts
 *    的 AgentStatus 事件签名；api-proxy.ts 消费示例）。最初假定 `{ running, sessionId }`
 *    —— 已修正。
 * 5. ZapMomo 桥（src/dsh/event.rs）要求的请求字段是 snake_case：
 *    `{ type, session_id, title?, reason?, detail? }`，且 `reason` 必须是**字符串**
 *    （如 `"completed"`），传原始对象会被服务端宽容解析丢弃（field type drift）。
 *    最初假定 camelCase `sessionId` —— 已修正。
 *
 * ─────────────────────────────────────────────────────────────────────────────
 * 待真 dsh 实测核对（T17 端到端联调）
 * ─────────────────────────────────────────────────────────────────────────────
 * 下列事件在真实 dsh 运行时的载荷已按源码推断，但尚未实跑核对：
 * - `session/event` 第一参确实是 Session（有 `.id`）、第二参确实含 `type`/`data`；
 * - 一个 `turn/end` 周期内 `agent/status` 是否稳定地先发一次 `running`；
 * - `user/message` 的 `data.content` 块结构（text 块是否直接就是 `{type:'text',text}`）；
 * - `turn/end` 各 reason.kind 是否都能出现、`error` 时 `reason.error` 是否带 message/code。
 * 核对方法：先部署本插件并保持 `bridge()` 为空实现（或让 post() 只打日志）跑真任务，
 * 观察日志中的原始载荷，再开启 POST。
 */

import type { Context } from '@deepseek-ai/cordis'
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

export const name = 'zapmomo-bridge'

export interface Config {}

/** ZapMomo 桥发现文件路径（ZapMomo 侧 settings::get_settings_dir() 默认 `~/.zapmomo`）。 */
const BRIDGE_FILE = join(homedir(), '.zapmomo', 'runtime', 'dsh-bridge.json')

/** 发现文件内容：loopback 端口 + 鉴权 token。 */
interface BridgeInfo {
  port: number
  token: string
}

/** session/event 的第一参（dsh Session 的宽松视图）。 */
interface SessionLike {
  id: string
  [k: string]: unknown
}

/** session/event 的第二参（dsh SessionEvent 的宽松视图）。 */
interface SessionEventLike {
  type?: string
  data?: Record<string, unknown>
  [k: string]: unknown
}

/** agent/status 载荷的宽松视图：`agent` 由 fused dispatch 注入。 */
interface AgentStatusPayload {
  agent?: { id?: string }
  status?: string
  [k: string]: unknown
}

/**
 * 现读发现文件。ZapMomo 未运行 / 文件缺失 / 解析失败都返回 null —— 调用方静默跳过。
 * 每次发送前现读，避免缓存旧端口/token。
 */
function bridge(): BridgeInfo | null {
  try {
    return JSON.parse(readFileSync(BRIDGE_FILE, 'utf8')) as BridgeInfo
  } catch {
    return null
  }
}

/**
 * fire-and-forget POST：1s 超时、所有异常吞掉只留给 debug —— 插件绝不影响 dsh 宿主。
 * body 字段名对齐 ZapMomo `src/dsh/event.rs` 的 snake_case 契约：
 * `{ type, session_id, title?, reason?, detail? }`。
 */
function post(type: string, sessionId: string, extra: Record<string, unknown> = {}): void {
  const info = bridge()
  if (!info) return // ZapMomo 未运行：静默跳过
  const url = `http://127.0.0.1:${info.port}/dsh/events`
  fetch(url, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${info.token}`,
    },
    body: JSON.stringify({ type, session_id: sessionId, ...extra }),
    signal: AbortSignal.timeout(1000),
  }).catch(() => {
    // 桥不可达（ZapMomo 刚退出等）：吞掉，不影响 dsh
  })
}

/**
 * 从 UserMessage 的 data 里提取用户可见文本。
 * `data.content` 是 ContentBlock[]，用户文本在 `type === 'text'` 块的 `.text`；
 * 兜底处理 content 意外为字符串的情况。
 */
function extractText(data: Record<string, unknown>): string {
  const content = data.content
  if (Array.isArray(content)) {
    return content
      .filter((block): block is Record<string, unknown> =>
        typeof block === 'object' && block !== null)
      .filter(block => block.type === 'text')
      .map(block => String(block.text ?? ''))
      .join(' ')
      .trim()
  }
  if (typeof content === 'string') return content.trim()
  return ''
}

export function apply(ctx: Context, _config: Config) {
  const logger = ctx.logger('zapmomo-bridge')
  // 会话 -> 最近一条用户指令摘要（模板台词的 title；前 40 字符）
  const titleBySession = new Map<string, string>()

  // 心跳：ZapMomo「插件集成」页据此判定「插件在线」。启动即发一次，之后每
  // 15s 重发（ZapMomo 侧 45s 无心跳视为离线 = 3 个周期容错）。unref 保证
  // 定时器不阻塞 dsh 宿主退出；post 本身 fire-and-forget，绝不影响宿主。
  post('plugin-hello', 'plugin')
  const heartbeat = setInterval(() => post('plugin-hello', 'plugin'), 15_000)
  heartbeat.unref?.()
  logger.info('zapmomo-bridge 心跳已启动（15s 间隔）')

  ctx.on('session/event', (session: SessionLike, ev: SessionEventLike) => {
    const type = ev?.type
    const sessionId = session?.id
    const data = (ev?.data ?? {}) as Record<string, unknown>

    // user/message：记录本会话最近一条用户指令，作为任务标题
    if (type === 'user/message') {
      const text = extractText(data)
      if (text && sessionId) {
        titleBySession.set(sessionId, text.slice(0, 40))
      }
      return
    }

    // 其余类型只关心 turn/end（携带结束原因）
    if (type !== 'turn/end' || !sessionId) return

    const reasonObj = data.reason as Record<string, unknown> | undefined
    const reason = typeof reasonObj?.kind === 'string' ? reasonObj.kind : ''
    const title = titleBySession.get(sessionId)

    if (reason === 'completed') {
      post('task-finished', sessionId, { title, reason })
    } else if (reason === 'error') {
      // 错误详情在 reason.error（LlmFailure），不是 detail 字段
      const err = reasonObj?.error as Record<string, unknown> | undefined
      const detail = [err?.message, err?.code]
        .filter((part): part is string => typeof part === 'string' && part.length > 0)
        .join(' — ')
        .slice(0, 200)
      post('task-failed', sessionId, { title, reason, detail })
    } else if (reason) {
      // aborted / interrupted / max-tokens / blocked -> 中断
      post('task-interrupted', sessionId, { title, reason })
    } else {
      logger.debug('turn/end 缺 reason.kind，跳过:', JSON.stringify(ev))
    }
  })

  ctx.on('agent/status', (payload: AgentStatusPayload) => {
    // 只报「开始」：结束由 turn/end 携带原因上报，避免同一次结束推两条
    const agentId = payload?.agent?.id
    if (payload?.status === 'running' && agentId) {
      post('task-started', agentId, { title: titleBySession.get(agentId) })
    } else {
      logger.debug('agent/status ignored:', JSON.stringify(payload))
    }
  })
}
