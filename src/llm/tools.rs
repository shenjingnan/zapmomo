/// 工具运行时：注册工具定义 + 执行工具调用。
///
/// - `get_current_time`：基础工具（始终注册）。
/// - `run_command`：CLI 工具，执行 shell 命令（`cli_tools` 开启才注册，
///   配置 `[llm] cli_tools = true`）。
/// - `set_character_sprite`：角色形象切换（active 角色包带 `sprites/` 目录才注册，
///   动态探测，见 [`crate::companion_sprites`]）。
///
/// `run_command` 的安全边界：
/// - **默认关闭**：模型只能调用已注册的工具，不注册即不可达；
/// - **危险命令拦截**：fork bomb / rm -rf 根目录或 HOME / mkfs / dd 写设备 /
///   关机重启 / sudo 提权等灾难性模式直接拒绝；
/// - **超时终止**：默认 30s，超时强杀；
/// - **输出截断**：默认 8K 字符，防上下文爆炸；
/// - **失败即结果**：非零退出 / 超时 / 拦截 / 参数错误都作为工具结果文本返回
///   （而非中断 Agent Loop），模型可见并自行恢复。
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use crate::llm::error::LlmError;
use crate::llm::types::ToolDefinition;

/// run_command 默认超时
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// run_command 输出截断阈值（字符数）
const CMD_OUTPUT_MAX_CHARS: usize = 8192;
/// try_wait 轮询间隔
const CMD_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 临时输出文件名的全局序号（配合 pid 保证唯一）
static CMD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 危险命令拦截表：(归一化后的子串, 拦截原因)。
/// 归一化 = 小写化 + 折叠连续空白。拦截是保守兜底，不是安全沙箱——
/// 真正的边界是「默认关闭 + 用户显式开启」。
/// 注：rm -rf 不在此表，由 [`is_dangerous_rm`] 按目标精确判断
/// （放行 `rm -rf /tmp/xxx` 等常规用法，只拦根目录/HOME/通配符全删）。
const BLOCKED_PATTERNS: &[(&str, &str)] = &[
    (":(){", "fork bomb"),
    ("mkfs", "格式化文件系统"),
    ("of=/dev/", "dd 直写块设备"),
    ("> /dev/sd", "覆写块设备"),
    ("shutdown", "关机"),
    ("reboot", "重启"),
    ("poweroff", "关机"),
    ("halt", "停机"),
    ("sudo ", "sudo 提权（交互密码会挂起）"),
];

/// 工具运行时（硬编码工具列表；`cli_tools` 控制是否注册 run_command）。
pub struct ToolRuntime {
    cli_tools: bool,
    /// run_command 超时（测试可缩短）
    cmd_timeout: Duration,
}

impl ToolRuntime {
    pub fn new(cli_tools: bool) -> Self {
        Self {
            cli_tools,
            cmd_timeout: CMD_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_cmd_timeout(mut self, timeout: Duration) -> Self {
        self.cmd_timeout = timeout;
        self
    }

    /// 可用工具的定义（传给模型的 `tools` 参数）。
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = vec![ToolDefinition {
            name: "get_current_time".to_string(),
            description: "获取当前本地时间（ISO 8601 格式）。".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }];
        if self.cli_tools {
            defs.push(ToolDefinition {
                name: "run_command".to_string(),
                description: "在用户的电脑上执行一条 shell 命令，返回退出码与输出（stdout+stderr）。\
                    适用于查看文件、查询系统状态或用户明确要求的操作。命令有超时限制，输出会被截断；\
                    破坏性命令会被安全策略拦截。"
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "要执行的 shell 命令（Unix 经 sh -c，Windows 经 cmd /C）"
                        }
                    },
                    "required": ["command"]
                }),
            });
        }
        // 角色形象：active 角色包带 sprites/ 时动态注册（每轮探测，
        // 中途加图 / 切换伙伴下一轮自动生效）；文件名 stem 即形象语义。
        let sprites = crate::companion_sprites::list_active_sprites();
        if !sprites.is_empty() {
            let names: Vec<&str> = sprites.iter().map(|s| s.name.as_str()).collect();
            defs.push(ToolDefinition {
                name: "set_character_sprite".to_string(),
                description: format!(
                    "切换你的桌面形象（立绘/表情）。当对话情绪发生明显变化时调用，\
                    可与文字回复在同一轮一起发出。可用形象：{}。\
                    传 \"default\" 恢复默认立绘。",
                    names.join(", ")
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "形象名，必须是可用形象之一，或 default"
                        }
                    },
                    "required": ["name"]
                }),
            });
        }
        defs
    }

    /// 执行工具，返回文本结果。
    pub fn execute(&self, name: &str, arguments: &str) -> Result<String, LlmError> {
        match name {
            "get_current_time" => Ok(chrono::Local::now().to_rfc3339()),
            "run_command" if self.cli_tools => Ok(self.run_command(arguments)),
            "set_character_sprite" => Ok(crate::companion_sprites::apply_tool_call(arguments)),
            other => Err(LlmError::InferenceFailed(format!("未知工具: {other}"))),
        }
    }

    /// 执行 shell 命令；一切结果（含失败/超时/拦截）都转为工具结果文本，
    /// 让模型可见并自行恢复，而不是中断 Agent Loop。
    fn run_command(&self, arguments: &str) -> String {
        let command = serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|v| v.get("command")?.as_str().map(str::to_string));
        let Some(command) = command else {
            return "参数错误：缺少字符串字段 command".to_string();
        };
        if let Some(reason) = blocked_reason(&command) {
            return format!("命令被安全策略拦截（{reason}），未执行");
        }

        // 输出写入临时文件而非管道：避免子进程写满管道缓冲（~64KB）后阻塞，
        // 与父进程 try_wait 轮询互相死锁
        let seq = CMD_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let out_path =
            std::env::temp_dir().join(format!("zapmomo-cmd-{}-{seq}.log", std::process::id()));
        let out_file = match std::fs::File::create(&out_path) {
            Ok(f) => f,
            Err(e) => return format!("创建输出临时文件失败：{e}"),
        };
        let err_file = match out_file.try_clone() {
            Ok(f) => f,
            Err(e) => return format!("创建输出临时文件失败：{e}"),
        };

        #[cfg(unix)]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&command);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&command);
            c
        };
        cmd.stdout(Stdio::from(out_file))
            .stderr(Stdio::from(err_file));

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_file(&out_path);
                return format!("命令启动失败：{e}");
            }
        };
        let deadline = Instant::now() + self.cmd_timeout;
        let timed_out = loop {
            match child.try_wait() {
                Ok(Some(_)) => break false,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    break true;
                }
                Ok(None) => std::thread::sleep(CMD_POLL_INTERVAL),
                Err(e) => {
                    let _ = std::fs::remove_file(&out_path);
                    return format!("命令状态查询失败：{e}");
                }
            }
        };
        // 回收子进程（kill 后也要 wait 防僵尸）；注意 sh -c 派生的孙进程可能残留
        let status = child.wait().ok().and_then(|s| s.code());

        let output = std::fs::read_to_string(&out_path).unwrap_or_default();
        let _ = std::fs::remove_file(&out_path);

        let mut result = String::new();
        if timed_out {
            result.push_str(&format!(
                "命令执行超时（{}s），已强制终止。\n",
                self.cmd_timeout.as_secs()
            ));
        } else {
            result.push_str(&format!("退出码：{}\n", status.unwrap_or(-1)));
        }
        result.push_str(&truncate_output(&output));
        result
    }
}

/// 危险命令检测：返回 Some(拦截原因) 表示应拒绝执行。
fn blocked_reason(command: &str) -> Option<&'static str> {
    // 小写化 + 折叠连续空白，提升模式命中稳定性
    let normalized = command
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if is_dangerous_rm(&normalized) {
        return Some("递归删除根目录/HOME/通配符全量删除");
    }
    BLOCKED_PATTERNS
        .iter()
        .find(|(pat, _)| normalized.contains(pat))
        .map(|(_, reason)| *reason)
}

/// rm -rf/-fr 的目标精确判断：只对根目录、HOME、通配符全量删除返回 true，
/// 放行 `rm -rf /tmp/xxx`、`rm -rf ./build` 等常规用法。
fn is_dangerous_rm(normalized: &str) -> bool {
    for pat in ["rm -rf ", "rm -fr "] {
        if let Some(pos) = normalized.find(pat) {
            let target = normalized[pos + pat.len()..]
                .split_whitespace()
                .next()
                .unwrap_or("");
            if matches!(
                target,
                "/" | "//" | "/*" | "~" | "~/" | "~/*" | "$home" | "$home/" | "$home/*" | "*"
            ) {
                return true;
            }
        }
    }
    false
}

/// 输出截断：超过阈值保留前 N 字符并附截断标记。
fn truncate_output(output: &str) -> String {
    if output.chars().count() <= CMD_OUTPUT_MAX_CHARS {
        return output.to_string();
    }
    let kept: String = output.chars().take(CMD_OUTPUT_MAX_CHARS).collect();
    format!("{kept}\n...（输出过长，已截断）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    /// 在当前 temp HOME 下导入带 sprites/ 的角色包（自动设为 active）。
    fn import_pack_with_sprites(home: &std::path::Path) {
        let src = home.join("furina");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("character.md"), "# 芙宁娜\n\n你是芙宁娜。\n").unwrap();
        std::fs::write(src.join("character.png"), b"\x89PNG\r\n\x1a\n fake").unwrap();
        std::fs::create_dir_all(src.join("sprites")).unwrap();
        std::fs::write(src.join("sprites/happy.png"), b"png").unwrap();
        std::fs::write(src.join("sprites/angry.png"), b"png").unwrap();
        crate::companion::import_character_from_dir(&src).unwrap();
    }

    // ---------- 角色形象工具（sprites/ 动态注册） ----------

    #[test]
    fn test_sprite_tool_registered_only_with_sprites() {
        run_with_temp_home(|home| {
            let rt = ToolRuntime::new(false);
            // 无 active 伙伴 → 不注册
            assert!(
                rt.definitions()
                    .iter()
                    .all(|d| d.name != "set_character_sprite")
            );

            // 角色包带 sprites/ → 注册，描述内联 stem 列表
            import_pack_with_sprites(home);
            let defs = rt.definitions();
            let def = defs
                .iter()
                .find(|d| d.name == "set_character_sprite")
                .expect("角色包带 sprites 时应注册形象工具");
            assert!(
                def.description.contains("happy") && def.description.contains("angry"),
                "描述应内联可用形象：{}",
                def.description
            );
            assert_eq!(def.parameters["required"], serde_json::json!(["name"]));
            assert_eq!(def.parameters["properties"]["name"]["type"], "string");
        });
    }

    #[test]
    fn test_sprite_tool_not_registered_without_sprites() {
        run_with_temp_home(|home| {
            // 角色包但没有 sprites/ 目录 → 不注册
            let src = home.join("furina");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("character.md"), "# 芙宁娜\n").unwrap();
            std::fs::write(src.join("character.png"), b"\x89PNG\r\n\x1a\n fake").unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();

            let rt = ToolRuntime::new(false);
            assert!(
                rt.definitions()
                    .iter()
                    .all(|d| d.name != "set_character_sprite")
            );

            // GIF 伙伴同样不注册
            let gif = home.join("舞.gif");
            std::fs::write(&gif, b"GIF89a\x01\x00\x01\x00\x00").unwrap();
            let (gif_model, _) = crate::companion::import_gif_from_file(&gif).unwrap();
            crate::companion::set_active(&gif_model.id).unwrap();
            assert!(
                rt.definitions()
                    .iter()
                    .all(|d| d.name != "set_character_sprite")
            );
        });
    }

    #[test]
    fn test_execute_set_character_sprite() {
        run_with_temp_home(|home| {
            import_pack_with_sprites(home);
            let rt = ToolRuntime::new(false);

            let out = rt
                .execute("set_character_sprite", r#"{"name":"happy"}"#)
                .unwrap();
            assert!(out.contains("已切换"), "实际：{out}");

            let out = rt
                .execute("set_character_sprite", r#"{"name":"default"}"#)
                .unwrap();
            assert!(out.contains("default"), "实际：{out}");

            // 未知名走「失败即结果」，返回提示文本而非 Err
            let out = rt
                .execute("set_character_sprite", r#"{"name":"nope"}"#)
                .unwrap();
            assert!(out.contains("未找到"), "实际：{out}");
        });
    }

    // ---------- 注册门控 ----------

    #[test]
    fn test_definitions_without_cli() {
        // HOME 隔离：definitions 会探测角色包 sprites，不能依赖真机环境
        run_with_temp_home(|_home| {
            let rt = ToolRuntime::new(false);
            let defs = rt.definitions();
            assert_eq!(defs.len(), 1);
            assert_eq!(defs[0].name, "get_current_time");
        });
    }

    #[test]
    fn test_definitions_with_cli() {
        run_with_temp_home(|_home| {
            let rt = ToolRuntime::new(true);
            let defs = rt.definitions();
            assert_eq!(defs.len(), 2);
            assert_eq!(defs[1].name, "run_command");
            assert_eq!(
                defs[1].parameters["required"],
                serde_json::json!(["command"])
            );
        });
    }

    #[test]
    fn test_run_command_rejected_when_cli_disabled() {
        let rt = ToolRuntime::new(false);
        let err = rt
            .execute("run_command", r#"{"command":"echo hi"}"#)
            .err()
            .unwrap();
        assert!(err.to_string().contains("未知工具"), "实际错误：{err}");
    }

    // ---------- 基础工具 ----------

    #[test]
    fn test_get_current_time() {
        let rt = ToolRuntime::new(false);
        let out = rt.execute("get_current_time", "{}").unwrap();
        assert!(out.contains('T'), "应为 RFC 3339 格式：{out}");
    }

    #[test]
    fn test_unknown_tool() {
        let rt = ToolRuntime::new(true);
        let err = rt.execute("no_such_tool", "{}").err().unwrap();
        assert!(err.to_string().contains("未知工具"), "实际错误：{err}");
    }

    // ---------- 危险命令拦截 ----------

    #[test]
    fn test_blocked_commands() {
        let rt = ToolRuntime::new(true);
        for cmd in [
            "rm -rf /",
            "rm  -rf   /*", // 连续空白归一化后仍命中
            "rm -rf ~",
            "rm -rf $HOME",
            "sudo ls /root",
            "shutdown -h now",
            "reboot",
            "mkfs.ext4 /dev/sda1",
            "dd if=/dev/zero of=/dev/sda",
            ":(){ :|:& };:",
        ] {
            let args = serde_json::json!({"command": cmd}).to_string();
            let out = rt.execute("run_command", &args).unwrap();
            assert!(out.contains("拦截"), "{cmd} 应被拦截，实际：{out}");
        }
    }

    #[test]
    fn test_normal_command_not_blocked() {
        assert!(blocked_reason("ls -la").is_none());
        assert!(blocked_reason("git status").is_none());
        assert!(blocked_reason("cat /tmp/a.log").is_none());
        // rm -rf 精确目标判断：常规路径放行
        assert!(blocked_reason("rm -rf /tmp/zapmomo-test").is_none());
        assert!(blocked_reason("rm -rf ./build").is_none());
        // 保守策略：哪怕只是 echo 文本里出现 shutdown 也拦截（拦截成本远低于漏放）
        assert!(blocked_reason("echo shutdown 只是文本").is_some());
    }

    // ---------- 参数错误 ----------

    #[test]
    fn test_run_command_bad_arguments() {
        let rt = ToolRuntime::new(true);
        let out = rt.execute("run_command", "{}").unwrap();
        assert!(out.contains("参数错误"), "实际：{out}");
        let out = rt.execute("run_command", "not json").unwrap();
        assert!(out.contains("参数错误"), "实际：{out}");
    }

    // ---------- 真实执行（Unix shell） ----------

    #[cfg(unix)]
    #[test]
    fn test_run_command_echo() {
        let rt = ToolRuntime::new(true);
        let out = rt
            .execute("run_command", r#"{"command":"echo hello-zapmomo"}"#)
            .unwrap();
        assert!(out.contains("退出码：0"), "实际：{out}");
        assert!(out.contains("hello-zapmomo"), "实际：{out}");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_command_nonzero_exit_and_stderr() {
        let rt = ToolRuntime::new(true);
        let out = rt
            .execute(
                "run_command",
                r#"{"command":"ls /nonexistent-zapmomo-xyz"}"#,
            )
            .unwrap();
        assert!(!out.contains("退出码：0"), "实际：{out}");
        assert!(out.contains("nonexistent-zapmomo-xyz"), "实际：{out}");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_command_timeout() {
        let rt = ToolRuntime::new(true).with_cmd_timeout(Duration::from_millis(300));
        let out = rt
            .execute("run_command", r#"{"command":"sleep 5"}"#)
            .unwrap();
        assert!(out.contains("超时"), "实际：{out}");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_command_output_truncated() {
        let rt = ToolRuntime::new(true);
        // 产生 200KB 输出，应被截断到 8K 字符 + 标记
        let out = rt
            .execute(
                "run_command",
                r#"{"command":"awk 'BEGIN{for(i=0;i<20000;i++)printf \"xxxxxxxxxx\"}'"}"#,
            )
            .unwrap();
        assert!(out.contains("已截断"), "实际长度 {} 未截断", out.len());
        assert!(out.len() < 9000, "截断后仍过长：{}", out.len());
    }
}
