//! 「命令行用法错误」标记与退出码判定。
//!
//! 退出码 2 与 1 的分界不是「谁报的错」，而是**错误来自哪里**：
//!
//! - 2：命令行本身用错了 —— clap 的结构性错误（缺参数、未知选项），以及 `gg`
//!   自己对参数取值的校验（`--lang jp`、含空格的命令名、空的搜索关键词）。
//! - 1：命令行没问题，但操作没能完成 —— 配置文件非法、文件不可写、外部命令
//!   失败等环境/资源问题。
//!
//! 前半段由 clap 直接给出退出码 2，不需要这里的标记；后半段是 `gg` 自己的校验，
//! 错误经 `anyhow` 向上传播，`main` 无法只凭类型区分。于是用 [`UsageError`]
//! 显式标记，由 [`exit_code_for`] 在链上查找。

/// 命令行用法错误的退出码，与 clap 自身的约定保持一致。
pub const EXIT_USAGE_ERROR: u8 = 2;

/// 运行时错误的退出码（`std::process::ExitCode::FAILURE` 的值）。
const EXIT_RUNTIME_ERROR: u8 = 1;

/// 标记「这是命令行用法错误」。
///
/// 只承载一条已经本地化好的文案；退出码由类型本身表达，与文案无关。
#[derive(Debug)]
pub struct UsageError(String);

impl UsageError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// 构造一个被标记为用法错误的 [`anyhow::Error`]。
pub fn usage(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(UsageError::new(message))
}

/// 根据错误链判定退出码。
///
/// 用 `chain()` 而不是只看最外层：校验失败常被 `with_context` 包过一层
/// （如 `rm` 的路径穿越），只看外层会漏判。
pub fn exit_code_for(err: &anyhow::Error) -> u8 {
    if err.chain().any(|cause| cause.is::<UsageError>()) {
        EXIT_USAGE_ERROR
    } else {
        EXIT_RUNTIME_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn plain_errors_are_runtime_errors() {
        let err = anyhow::anyhow!("磁盘炸了");
        assert_eq!(exit_code_for(&err), EXIT_RUNTIME_ERROR);
    }

    #[test]
    fn usage_errors_report_exit_code_2() {
        let err = usage("命令名不能含空格");
        assert_eq!(exit_code_for(&err), EXIT_USAGE_ERROR);
        assert_eq!(format!("{err:#}"), "命令名不能含空格");
    }

    /// 校验失败常被 `with_context` 包一层，标记必须能穿透。
    #[test]
    fn usage_marker_survives_context_wrapping() {
        let err = Err::<(), _>(usage("路径字符非法"))
            .context("无法处理这个笔记")
            .expect_err("必须失败");

        assert_eq!(exit_code_for(&err), EXIT_USAGE_ERROR);
        assert_eq!(format!("{err:#}"), "无法处理这个笔记: 路径字符非法");
    }

    /// `UsageError` 只是标记，不得被当成普通运行时错误吞掉。
    #[test]
    fn runtime_error_cause_does_not_leak_the_usage_code() {
        let err = Err::<(), _>(anyhow::anyhow!("权限不足"))
            .context("无法删除笔记")
            .expect_err("必须失败");

        assert_eq!(exit_code_for(&err), EXIT_RUNTIME_ERROR);
    }
}
