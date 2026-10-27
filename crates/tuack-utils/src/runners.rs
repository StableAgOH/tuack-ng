//! 运行器实现，以及执行子进程时共用的输入注入、标准流接线与产物收集。

pub mod cpp;
pub mod general;

use std::process::{Command as StdCommand, Stdio};

use tempfile::TempDir;

use crate::prelude::*;
use crate::process::SuperviseOutcome;
use crate::utils::KeepAliveReader;
use tuack_lib::data::Reader;
use tuack_lib::utils::compiler::{IoMode, RunError, RunResult, RunStatus};

/// 本次运行无法完成的结果：没有用时、内存与产物
pub(crate) fn failed_run(error: RunError) -> RunResult {
    RunResult {
        status: RunStatus::Incomplete(error),
        time: None,
        memory: None,
        output: None,
        stderr: None,
    }
}

/// 把输入流写入临时目录，供标准输入或文件 IO 读取
pub(crate) fn inject_input(
    tmp_dir: &Path,
    io_mode: &IoMode,
    input: &mut dyn Reader,
) -> std::io::Result<()> {
    let path = match io_mode {
        IoMode::Stdio => tmp_dir.join("pipe_stdin"),
        IoMode::File { input_name, .. } => tmp_dir.join(input_name),
    };
    let mut file = std::fs::File::create(path)?;
    std::io::copy(input, &mut file)?;
    Ok(())
}

/// 把命令的标准输入/输出/错误接到临时文件；文件 IO 时标准流置空
pub(crate) fn wire_stdio(
    tmp_dir: &Path,
    io_mode: &IoMode,
    cmd: &mut StdCommand,
) -> std::io::Result<()> {
    match io_mode {
        IoMode::Stdio => {
            let stdin = std::fs::File::open(tmp_dir.join("pipe_stdin"))?;
            let stdout = std::fs::File::create(tmp_dir.join("pipe_stdout"))?;
            cmd.stdin(Stdio::from(stdin));
            cmd.stdout(Stdio::from(stdout));
        }
        IoMode::File { .. } => {
            cmd.stdin(Stdio::null());
            cmd.stdout(Stdio::null());
        }
    }
    let stderr = std::fs::File::create(tmp_dir.join("pipe_stderr"))?;
    cmd.stderr(Stdio::from(stderr));
    Ok(())
}

/// 一次运行的产物
pub(crate) struct CollectedOutput {
    /// 输出流；输出文件不存在时为 `None`
    pub(crate) output: Option<Box<dyn Reader>>,
    /// 标准错误输出流
    pub(crate) stderr: Box<dyn Reader>,
}

/// 读取标准错误与输出流；产物流持有临时目录，离开运行器后仍可读
pub(crate) fn collect_output(
    tmp_dir: &Arc<TempDir>,
    io_mode: &IoMode,
) -> std::io::Result<CollectedOutput> {
    let dir = tmp_dir.path();
    let stderr = KeepAliveReader::new(
        std::fs::File::open(dir.join("pipe_stderr"))?,
        Arc::clone(tmp_dir),
    );
    let output_path = match io_mode {
        IoMode::Stdio => dir.join("pipe_stdout"),
        IoMode::File { output_name, .. } => dir.join(output_name),
    };
    let output: Option<Box<dyn Reader>> = if output_path.exists() {
        Some(Box::new(KeepAliveReader::new(
            std::fs::File::open(&output_path)?,
            Arc::clone(tmp_dir),
        )))
    } else {
        None
    };
    Ok(CollectedOutput {
        output,
        stderr: Box::new(stderr),
    })
}

/// 用监督结果组装运行结果；本次运行无法完成时不收集产物
pub(crate) fn finish_run(
    outcome: SuperviseOutcome,
    tmp_dir: &Arc<TempDir>,
    io_mode: &IoMode,
) -> std::io::Result<RunResult> {
    let status = match outcome.status {
        RunStatus::Incomplete(error) => return Ok(failed_run(error)),
        status @ (RunStatus::Success
        | RunStatus::NonZeroExit(_)
        | RunStatus::TimeLimitExceeded
        | RunStatus::MemoryLimitExceeded) => status,
    };
    let collected = collect_output(tmp_dir, io_mode)?;
    Ok(RunResult {
        status,
        time: outcome.time,
        memory: outcome.memory,
        output: collected.output,
        stderr: Some(collected.stderr),
    })
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use super::finish_run;
    use crate::process::SuperviseOutcome;
    use tempfile::TempDir;
    use tuack_lib::utils::compiler::{IoMode, RunError, RunStatus};

    /// 本次运行无法完成时不收集产物：没有输出流与标准错误流
    #[test]
    fn incomplete_run_has_no_products() {
        let tmp_dir = Arc::new(TempDir::with_prefix("tuack-ng-test-").expect("创建临时目录失败"));
        let outcome = SuperviseOutcome {
            status: RunStatus::Incomplete(RunError::Spawn {
                program: "missing".to_string(),
                source: io::Error::new(io::ErrorKind::NotFound, "missing"),
            }),
            time: None,
            memory: None,
        };

        let run = finish_run(outcome, &tmp_dir, &IoMode::Stdio).expect("组装结果失败");

        assert!(run.output.is_none());
        assert!(run.stderr.is_none());
    }
}
