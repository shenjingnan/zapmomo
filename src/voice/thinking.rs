/// 流式思考块过滤（`<think>...</think>`）。
///
/// 从 `session.rs` 的 `ReplyAccumulator` 内部逻辑提取，供语音会话与 dsh 播报
/// （`dsh::narrate`）共用：凡把 LLM 流式 token 拼成完整文本的场景都需要它——
/// `enable_thinking=false` 的空引导对部分模型无效，思考块会混进正文。
/// 语义不变：思考块内容不送 TTS、不入历史、不上屏。
///
/// 思考块标签。`<think>` 块内的内容（思考过程）不送 TTS、不入历史、不上屏。
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// 流式思考块过滤器：丢弃 `<think>...</think>` 之间的内容。
///
/// 流式 token 可能把标签拆成多段（如 `<th` + `ink>`），因此保留尾部若干字节
/// （最长标签长度 - 1）等待补全；未闭合的思考块在 `finish` 时整体丢弃。
#[derive(Default)]
pub(crate) struct ThinkingFilter {
    in_think: bool,
    buffer: String,
}

impl ThinkingFilter {
    /// 吸收增量，返回过滤掉思考块后的可见文本。
    pub(crate) fn feed(&mut self, delta: &str) -> String {
        self.buffer.push_str(delta);
        let mut out = String::new();
        loop {
            if self.in_think {
                match self.buffer.find(THINK_CLOSE) {
                    Some(pos) => {
                        self.buffer.drain(..pos + THINK_CLOSE.len());
                        self.in_think = false;
                        // 继续处理 </think> 之后的内容
                    }
                    None => {
                        // 思考块内：只保留可能是 `</think>` 前缀的尾部，其余丢弃
                        let keep = tag_prefix_tail(&self.buffer);
                        let cut = keep.unwrap_or(self.buffer.len());
                        self.buffer.drain(..cut);
                        break;
                    }
                }
            } else {
                match self.buffer.find(THINK_OPEN) {
                    Some(pos) => {
                        out.push_str(&self.buffer[..pos]);
                        self.buffer.drain(..pos + THINK_OPEN.len());
                        self.in_think = true;
                    }
                    None => {
                        // 正常文本：只保留可能是 `<think>` 前缀的尾部，其余输出
                        let keep = tag_prefix_tail(&self.buffer);
                        let cut = keep.unwrap_or(self.buffer.len());
                        out.push_str(&self.buffer[..cut]);
                        self.buffer.drain(..cut);
                        break;
                    }
                }
            }
        }
        out
    }

    /// 生成结束：返回可见残余（思考块未闭合则丢弃）。
    pub(crate) fn finish(&mut self) -> String {
        if self.in_think {
            self.buffer.clear();
            String::new()
        } else {
            std::mem::take(&mut self.buffer)
        }
    }
}

/// 若 `buffer` 末尾是 `<think>` / `</think>` 的前缀（跨 token 残片），返回其起始
/// 字节位置（需保留等待补全）；否则返回 `None`（整段可安全输出/丢弃）。
///
/// 只检查标签前缀，因此正常文本**不会**被尾部延迟截断。
fn tag_prefix_tail(buffer: &str) -> Option<usize> {
    const MAX: usize = THINK_CLOSE.len() - 1; // 最长标签 - 1
    let candidates = [THINK_OPEN, THINK_CLOSE];
    // 从后往前枚举 char 边界，检查后缀是否为标签前缀
    for (i, _) in buffer.char_indices().rev() {
        let suffix = &buffer[i..];
        if suffix.len() > MAX {
            break;
        }
        if candidates.iter().any(|tag| tag.starts_with(suffix)) {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_filter_multibyte_safety() {
        // 思考块内夹杂多字节中文，切分不得 panic（char 边界安全）
        let mut f = ThinkingFilter::default();
        let out = f.feed("<think>中文思考内容很长很长</think>可见");
        assert_eq!(out, "可见");
    }
}
