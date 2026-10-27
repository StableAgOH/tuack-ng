use std::io;
use std::time::Duration;

use crate::data::Reader;
use crate::prelude::*;

/// 运行器元信息
pub struct RunnerManifest {
    /// 是否支持交互式题目
    pub interactive: bool,
}

/// 进程资源限制
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// 时间限制，`None` 表示不限。
    pub time_limit: Option<Duration>,
    /// 内存限制（字节），`None` 表示不限。
    pub memory_limit: Option<u64>,
}

impl ResourceLimits {
    pub fn unlimited() -> Self {
        Self {
            time_limit: None,
            memory_limit: None,
        }
    }

    pub fn new(time_limit: Duration, memory_limit: u64) -> Self {
        Self {
            time_limit: Some(time_limit),
            memory_limit: Some(memory_limit),
        }
    }
}

/// 进程结束状态
#[derive(Debug)]
pub enum RunStatus {
    Success,
    /// 非零退出，携带退出码
    NonZeroExit(i32),
    TimeLimitExceeded,
    MemoryLimitExceeded,
    /// 本次运行无法完成：进程创建、输入注入或产物收集失败
    Incomplete(RunError),
}

/// [`Runner::execute`] 的返回结果
pub struct RunResult {
    pub status: RunStatus,
    /// 运行时间；本次运行无法完成时为 `None`。
    pub time: Option<Duration>,
    /// 峰值内存（字节）；未采到读数或本次运行无法完成时为 `None`。
    pub memory: Option<u64>,
    /// 程序输出流；`None` 表示输出文件不存在（文件 IO 程序未写输出）。
    pub output: Option<Box<dyn Reader>>,
    /// 程序标准错误流；本次运行无法完成时为 `None`
    pub stderr: Option<Box<dyn Reader>>,
}

/// IO 模式
#[derive(Debug, Clone)]
pub enum IoMode {
    /// 标准 IO
    Stdio,
    /// 文件 IO
    File {
        /// 输入文件名
        input_name: String,
        /// 输出文件名
        output_name: String,
    },
}

/// 一次运行的输入
pub struct RunSpec {
    /// 资源限制
    pub limits: ResourceLimits,
    /// IO 模式
    pub io_mode: IoMode,
    /// 输入内容：标准 IO 写入其标准输入，文件 IO 写入其输入文件
    pub input: Box<dyn Reader>,
}

/// 子进程监督失败的原因
#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    /// 子进程已被回收，取不到 PID
    #[error("无法获取子进程 PID")]
    MissingPid,
    /// 两轮都未能发送终止信号
    #[error("终止子进程 {pid} 失败：{source}")]
    Kill {
        pid: u32,
        #[source]
        source: io::Error,
    },
    /// 发出终止信号后进程未在等待期限内退出
    #[error("等待子进程 {pid} 退出超时")]
    WaitTimeout { pid: u32 },
    /// 等待子进程退出失败
    #[error("等待子进程退出失败：{0}")]
    Wait(#[source] io::Error),
    /// 回收子进程失败
    #[error("回收子进程 {pid} 失败：{source}")]
    Reap {
        pid: u32,
        #[source]
        source: io::Error,
    },
}

/// 本次运行无法完成的原因
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// 程序无法启动：可执行文件不存在或进程创建失败
    #[error("无法启动程序 {program}：{source}")]
    Spawn {
        /// 启动的程序
        program: String,
        #[source]
        source: io::Error,
    },
    /// 注入输入失败：创建输入文件或写入输入流失败
    #[error("注入输入失败：{0}")]
    Input(#[source] io::Error),
    /// 连接标准流失败：创建或打开标准输入/输出/错误文件失败
    #[error("连接标准流失败：{0}")]
    Stdio(#[source] io::Error),
    /// 收集运行产物失败：读取标准错误或打开输出文件失败
    #[error("读取运行产物失败：{0}")]
    Output(#[source] io::Error),
}

/// 运行器无法完成执行请求的原因
///
/// [`RunnerError::Command`] 发生在启动子进程之前，属配置或运行器错误；
/// [`RunnerError::Monitor`] 表示无法确认子进程状态或资源限制，不能继续评测。
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// 运行命令无法构造：语言模板渲染或解析失败
    #[error("无法构造运行命令：{0}")]
    Command(String),
    /// 子进程监督失败，无法保证资源限制
    #[error(transparent)]
    Monitor(#[from] MonitorError),
}

/// 运行器：编译 + 资源限制执行。
pub trait Runner: Send {
    /// 获取运行器元数据
    fn manifest(&self) -> RunnerManifest;

    /// 运行前准备（如编译）；须先于 `execute` 调用
    ///
    /// # Errors
    ///
    /// 准备失败时返回 `Err`：临时目录创建或源文件拷贝失败、编译命令构造失败、
    /// 编译器进程无法启动，或编译以非零码退出（编译错误输出在错误消息里）。
    fn prepare(&mut self) -> Result<()>;

    /// 启用交互模式：指定 grader 与头文件，须在 `prepare` 前调用
    ///
    /// # Panics
    ///
    /// 运行器不支持交互式题目（[`RunnerManifest::interactive`] 为 `false`）时
    /// panic；调用方须先经 [`Runner::manifest`] 确认支持。
    fn set_interactive(&mut self, grader_file: &Path, header_file: &Path) -> Result<()>;

    /// 运行结束后的清理；调用后不保证立即释放磁盘占用，实际回收由实现决定。
    fn cleanup(&mut self) -> Result<()>;

    /// 按 `spec` 执行一次程序。
    ///
    /// 程序无法启动、输入注入或产物收集失败，以及超时/超内存都不算错误，经返回值的
    /// [`RunStatus`] 报告。
    ///
    /// # Errors
    ///
    /// 运行器自身问题返回 [`RunnerError`]。
    fn execute(&mut self, spec: RunSpec) -> Result<RunResult, RunnerError>;
}
