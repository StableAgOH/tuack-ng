use std::io::Read;

use crate::data::{DataMut, DmkItem, Reader};
use crate::prelude::*;
use crate::utils::compiler::{IoMode, ResourceLimits, RunSpec, RunStatus, Runner};
use crate::utils::testlib::{Generator, Validator, ValidatorResult};

/// 失败诊断展示的标准错误字符数上限
const STDERR_EXCERPT_CHARS: usize = 100;

/// 读取标准错误开头的一小段，供失败诊断展示
fn stderr_excerpt(stderr: Option<&mut dyn Reader>) -> String {
    let Some(stderr) = stderr else {
        return String::new();
    };
    let mut bytes = Vec::new();
    // UTF-8 单字符最多 4 字节，多读一个字节用于判断是否截断
    if stderr
        .take((STDERR_EXCERPT_CHARS * 4 + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut excerpt: String = text.chars().take(STDERR_EXCERPT_CHARS).collect();
    if text.chars().count() > STDERR_EXCERPT_CHARS {
        excerpt.push_str("...");
    }
    excerpt
}

/// 数据生成会话
pub struct DmkSession<'a> {
    /// 已 prepare 的标程运行器
    runner: &'a mut dyn Runner,
    /// 已 prepare 的数据生成器
    generator: &'a mut dyn Generator,
    /// 已 prepare 的输入校验器（可为空）
    validator: Option<&'a dyn Validator>,
    /// 标程运行的 IO 模式
    io_mode: IoMode,
}

impl<'a> DmkSession<'a> {
    /// 创建会话。须保证各组件已完成 `prepare`（[`Runner::prepare`]、
    /// [`Generator::prepare`]、[`Validator::prepare`]）。
    pub fn new(
        runner: &'a mut dyn Runner,
        generator: &'a mut dyn Generator,
        validator: Option<&'a dyn Validator>,
        io_mode: IoMode,
    ) -> Self {
        Self {
            runner,
            generator,
            validator,
            io_mode,
        }
    }

    /// 生成单点输入：跑生成器写出 `item` 的输入，再用校验器复核
    ///
    /// 未提供校验器时跳过复核。
    ///
    /// # Errors
    ///
    /// 生成器运行失败、输入写入失败、读回输入失败、校验器无法运行，或输入未通过校验
    /// 时返回 `Err`。
    pub fn gen_input(&self, item: &dyn DmkItem, seed: u64) -> Result<()> {
        let stream = self.generator.run(item.args().clone(), seed)?;
        item.write_input(stream)?;

        let Some(validator) = self.validator else {
            return Ok(());
        };
        let mut input = item.input()?;
        match validator.validate(&mut *input)? {
            ValidatorResult::Ok => Ok(()),
            ValidatorResult::Invalid(message) => bail!("输入校验失败：{}", message),
        }
    }

    /// 用标程生成单点输出：以 `item` 的输入跑标程，把输出写回 `item`
    ///
    /// # Errors
    ///
    /// 读取输入或写入输出失败、标程未能完成本次运行、非正常退出或未产出输出时
    /// 返回 `Err`；非正常退出的消息附带标准错误的前 100 个字符。
    pub fn gen_output(&mut self, item: &dyn DataMut) -> Result<()> {
        let input = item.input()?;

        let mut result = self.runner.execute(RunSpec {
            limits: ResourceLimits::unlimited(),
            io_mode: self.io_mode.clone(),
            input,
        })?;

        match result.status {
            RunStatus::Success => {}
            RunStatus::Incomplete(error) => bail!("标程运行失败：{error:#}"),
            RunStatus::NonZeroExit(_)
            | RunStatus::TimeLimitExceeded
            | RunStatus::MemoryLimitExceeded => {
                // TODO: 引入错误类型，将 stderr 句柄传出
                let stderr = stderr_excerpt(result.stderr.as_deref_mut());
                if stderr.is_empty() {
                    bail!("标程运行失败");
                }
                bail!("标程运行失败\n标准错误输出：{stderr}");
            }
        }

        let output = match result.output {
            Some(out) => out,
            None => bail!("标程未生成输出"),
        };
        item.write_output(output)
    }
}
