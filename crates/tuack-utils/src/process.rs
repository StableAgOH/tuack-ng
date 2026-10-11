//! 子进程资源监控。
//!
//! 全仓库唯一的异步点：同步调用方经 [`ProcessSupervisor::supervise_blocking`] 进入
//! [`monitor_runtime`]，由 `tokio::select!` 同时等待子进程退出与内存采样，以便在确认
//! 超限的瞬间终止进程。
//!
//! 判定契约：只有确认超时或确认内存超限，才终止监控并结束子进程。单次采样取不到进程
//! （已退出，或本次刷新未命中）不构成超限证据，退出码与用时一律由 `child.wait()` 给出，
//! 避免把正常退出误判为 [`RunStatus::MemoryLimitExceeded`]。

use std::io;
use std::process::{Command, ExitStatus};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::process::Child;
use tokio::time::{Instant, sleep, timeout};

use crate::prelude::*;
use tuack_lib::utils::compiler::{ResourceLimits, RunStatus};

/// 监视器专用 runtime：供同步代码短时进入异步以监督子进程。
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// 内存采样与超限检查的节拍
const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);

/// 终止宽限：超出时限后再等这么久才终止子进程，容忍平台计时误差
const KILL_GRACE: Duration = Duration::from_millis(200);

/// 终止信号的重试间隔，也是单次等待退出的上界
const KILL_RETRY_DELAY: Duration = Duration::from_millis(200);

/// 返回监视器 runtime；首次调用时惰性构建单线程（`current_thread`）runtime。
pub fn monitor_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("构建监视 runtime 失败")
    })
}

/// 监控确认的资源超限类型
enum Violation {
    /// 超出时间限制
    Timeout,
    /// 超出内存限制
    Memory,
}

/// `tokio::select!` 先就绪的一方
enum Finished {
    /// 监控确认超限，进程需要终止
    Limit(Violation),
    /// 进程自行结束
    Exit(io::Result<ExitStatus>),
}

/// [`ProcessSupervisor::supervise`] 的结论
#[derive(Debug)]
pub struct SuperviseOutcome {
    /// 结束状态
    pub status: RunStatus,
    /// 用时；运行器内部错误时为 `None`（TLE/MLE 都记录终止时刻）
    pub time: Option<Duration>,
    /// 峰值内存（字节）；始终没采到读数或等待子进程失败时为 `None`
    pub memory: Option<u64>,
}

impl SuperviseOutcome {
    /// 超时结束：用时是确认超时并终止子进程的时刻，含 200ms 终止宽限，只是下界
    fn timeout(time: Duration, memory: Option<u64>) -> Self {
        Self {
            status: RunStatus::TimeLimitExceeded,
            time: Some(time),
            memory,
        }
    }

    /// 内存超限结束：用时是确认越限并终止子进程的时刻，仍反映程序跑了多久
    fn out_of_memory(time: Duration, memory: Option<u64>) -> Self {
        Self {
            status: RunStatus::MemoryLimitExceeded,
            time: Some(time),
            memory,
        }
    }
}

/// 子进程 TLE/MLE 监控器：持有 [`ResourceLimits`]，供
/// [`ProcessSupervisor::supervise_blocking`]/[`ProcessSupervisor::supervise`]
/// 监督子进程并在超限时终止
pub struct ProcessSupervisor {
    limits: ResourceLimits,
}

impl ProcessSupervisor {
    pub fn new(limits: ResourceLimits) -> Self {
        Self { limits }
    }

    /// 在监视器 runtime 上 `block_on` 一次 [`ProcessSupervisor::supervise`]，供同步调用方使用。
    ///
    /// `cmd` 的 stdin/stdout/stderr 由调用方预设；本函数只负责把它启动为子进程。
    ///
    /// # Panics
    ///
    /// 在异步上下文中调用时 panic（`block_on` 不能在 runtime 内执行）。
    ///
    /// # Errors
    ///
    /// 子进程启动失败，或 [`ProcessSupervisor::supervise`] 失败时返回 `Err`。
    pub fn supervise_blocking(self, cmd: Command) -> Result<SuperviseOutcome> {
        monitor_runtime().block_on(async move {
            let mut child = tokio::process::Command::from(cmd).spawn()?;
            self.supervise(&mut child).await
        })
    }

    /// 监督一个已 spawn 的子进程，返回 [`SuperviseOutcome`]。
    ///
    /// 监控循环与 `child.wait()` 竞争：前者只在确认超限时就绪，后者给出真实退出状态。
    /// 确认超限时终止子进程并回收，正常退出则按退出码归类。
    ///
    /// # Errors
    ///
    /// 子进程已被 `wait()` 轮询到完成（`Child::id()` 返回 `None`）时返回 `Err`；
    /// 确认超限后子进程仍未退出、或回收失败时也返回 `Err`（此时无法保证资源限制，
    /// 继续评测没有意义）。TLE/MLE 本身经 [`RunStatus`] 返回，不算错误。
    pub async fn supervise(self, child: &mut Child) -> Result<SuperviseOutcome> {
        let pid = child.id().context("无法获取进程 PID")?;
        let start = Instant::now();
        let peak = AtomicU64::new(0);
        let sampled = AtomicBool::new(false);

        let finished = tokio::select! {
            biased;
            violation = monitor(pid, start, &peak, &sampled, &self.limits) => Finished::Limit(violation),
            exit = child.wait() => Finished::Exit(exit),
        };

        let elapsed = start.elapsed();
        let memory = if sampled.load(Ordering::Relaxed) {
            Some(peak.load(Ordering::Relaxed))
        } else {
            None
        };
        let outcome = match finished {
            Finished::Limit(violation) => {
                kill_and_reap(child).await?;
                match violation {
                    Violation::Timeout => {
                        info!("进程超时：{}", elapsed.as_secs_f64());
                        SuperviseOutcome::timeout(elapsed, memory)
                    }
                    Violation::Memory => {
                        info!("进程内存超限：{}", elapsed.as_secs_f64());
                        SuperviseOutcome::out_of_memory(elapsed, memory)
                    }
                }
            }
            Finished::Exit(exit) => {
                let outcome = classify_exit(exit, elapsed, memory, &self.limits);
                info!(
                    "进程结束，耗时：{:?}, 峰值内存：{:?}, 状态：{:?}",
                    elapsed, outcome.memory, outcome.status
                );
                outcome
            }
        };

        Ok(outcome)
    }
}

/// 确认超限后终止子进程并回收
///
/// 最多两轮「发送终止信号 + 限时等待退出」，每轮等待上界 [`KILL_RETRY_DELAY`]。发送信号失败
/// 不等于终止失败（子进程可能已经退出），因此只记下该错误，是否已结束一律由 `wait()` 判定；
/// 两轮都未结束则返回 `Err`，不做无界等待。
///
/// # Errors
///
/// 两次等待退出都超时、或等待退出本身失败时返回 `Err`。
async fn kill_and_reap(child: &mut Child) -> Result<()> {
    let pid = child.id();
    let mut kill_error = None;

    for _ in 0..2 {
        kill_error = child.start_kill().err();

        // wait 是可取消安全的：超时后进入下一轮，再次发送终止信号
        if let Ok(status) = timeout(KILL_RETRY_DELAY, child.wait()).await {
            status.with_context(|| format!("回收子进程 {pid:?} 失败"))?;
            return Ok(());
        }
    }

    match kill_error {
        // 两轮都没能发送出终止信号
        Some(e) => Err(e).with_context(|| format!("终止子进程 {pid:?} 失败")),
        None => bail!("等待子进程 {pid:?} 退出超时"),
    }
}

/// 内存采样循环：确认超时或确认内存超限时返回对应的 [`Violation`]
///
/// `peak`/`sampled` 记下采样到的峰值内存以及是否至少采到过一次；采样取不到进程只跳过
/// 本次采样，不作为超限证据。
async fn monitor(
    pid: u32,
    start: Instant,
    peak: &AtomicU64,
    sampled: &AtomicBool,
    limits: &ResourceLimits,
) -> Violation {
    let time_limit = limits.time_limit.unwrap_or(Duration::MAX);
    let memory_limit = limits.memory_limit.unwrap_or(u64::MAX);
    let sys_pid = Pid::from_u32(pid);
    let mut sys = System::new();

    loop {
        // 先采样再等待：寿命短于一个采样节拍的进程也能取到内存读数
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        if let Some(process) = sys.process(sys_pid) {
            let memory = process.memory();
            peak.fetch_max(memory, Ordering::Relaxed);
            sampled.store(true, Ordering::Relaxed);
            if memory > memory_limit {
                return Violation::Memory;
            }
        }
        sleep(SAMPLE_INTERVAL).await;
        if start.elapsed() > time_limit.saturating_add(KILL_GRACE) {
            return Violation::Timeout;
        }
    }
}

/// 归类自行结束的进程：超出时限或内存仍按 TLE/MLE 处理，否则取真实退出状态
///
/// `memory` 是监控采到的峰值内存，`None` 表示始终没采到，此时不构成内存超限证据。
fn classify_exit(
    exit: io::Result<ExitStatus>,
    elapsed: Duration,
    memory: Option<u64>,
    limits: &ResourceLimits,
) -> SuperviseOutcome {
    if elapsed > limits.time_limit.unwrap_or(Duration::MAX) {
        return SuperviseOutcome::timeout(elapsed, memory);
    }
    if memory.is_some_and(|memory| memory > limits.memory_limit.unwrap_or(u64::MAX)) {
        return SuperviseOutcome::out_of_memory(elapsed, memory);
    }

    let status = match exit {
        Ok(status) if status.success() => RunStatus::Success,
        Ok(status) => RunStatus::NonZeroExit(status.code().unwrap_or(-1)),
        Err(e) => {
            return SuperviseOutcome {
                status: RunStatus::InternalError(e.into()),
                time: None,
                memory: None,
            };
        }
    };
    SuperviseOutcome {
        status,
        time: Some(elapsed),
        memory,
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::process::{Command, ExitStatus, Stdio};
    use std::time::Duration;

    use super::{ProcessSupervisor, SuperviseOutcome, classify_exit, monitor_runtime};
    use tuack_lib::utils::compiler::{ResourceLimits, RunStatus};

    fn limits(time_limit: Duration, memory_limit: u64) -> ResourceLimits {
        ResourceLimits::new(time_limit, memory_limit)
    }

    /// 占位退出状态：只用于验证在读取退出状态之前就已定论的分类
    fn unused_exit() -> io::Result<ExitStatus> {
        Err(io::Error::other("该分支不会读取退出状态"))
    }

    /// 超出时限后自行退出的进程仍判 TLE
    #[test]
    fn exit_after_time_limit_is_tle() {
        let outcome = classify_exit(
            unused_exit(),
            Duration::from_secs(2),
            None,
            &limits(Duration::from_secs(1), 1 << 30),
        );
        assert!(matches!(outcome.status, RunStatus::TimeLimitExceeded));
        // 超时也记录终止时刻，只是它含了 200ms 宽限
        assert_eq!(outcome.time, Some(Duration::from_secs(2)));
    }

    /// 峰值超过上限的进程即使自行退出也判 MLE，并保留峰值数值与用时
    #[test]
    fn peak_over_limit_is_mle() {
        let outcome = classify_exit(
            unused_exit(),
            Duration::from_millis(10),
            Some(4096),
            &limits(Duration::from_secs(1), 1024),
        );
        assert!(matches!(outcome.status, RunStatus::MemoryLimitExceeded));
        assert_eq!(outcome.memory, Some(4096));
        // MLE 的用时是程序实际跑到的时刻，不是「限时 + 宽限」
        assert_eq!(outcome.time, Some(Duration::from_millis(10)));
    }

    /// 始终没采到内存的正常退出必须判 Success 且内存未知，不能因缺少证据误判 MLE
    #[cfg(unix)]
    #[test]
    fn unsampled_normal_exit_is_success() {
        use std::os::unix::process::ExitStatusExt;

        let outcome = classify_exit(
            Ok(ExitStatus::from_raw(0)),
            Duration::from_millis(1),
            None,
            &limits(Duration::from_secs(1), 1 << 30),
        );
        assert!(matches!(outcome.status, RunStatus::Success));
        assert_eq!(outcome.time, Some(Duration::from_millis(1)));
        assert_eq!(outcome.memory, None);
    }

    /// 非零退出归为 RE 并带上退出码
    #[cfg(unix)]
    #[test]
    fn non_zero_exit_keeps_code() {
        use std::os::unix::process::ExitStatusExt;

        let outcome = classify_exit(
            Ok(ExitStatus::from_raw(3 << 8)),
            Duration::from_millis(1),
            None,
            &limits(Duration::from_secs(1), 1 << 30),
        );
        assert!(matches!(outcome.status, RunStatus::NonZeroExit(3)));
    }

    /// 构造一个立即正常退出的命令
    fn quick_exit_command() -> Command {
        #[cfg(windows)]
        let mut cmd = Command::new("cmd");
        #[cfg(windows)]
        cmd.args(["/C", "exit", "0"]);
        #[cfg(not(windows))]
        let mut cmd = Command::new("true");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd
    }

    /// 采样取不到进程（如短命进程在首个节拍前退出）不得被当成资源超限
    #[test]
    fn short_lived_process_exits_normally() {
        let limits = limits(Duration::from_secs(10), 1 << 30);
        for _ in 0..20 {
            let outcome = ProcessSupervisor::new(limits.clone())
                .supervise_blocking(quick_exit_command())
                .expect("启动子进程失败");
            assert!(
                matches!(&outcome.status, RunStatus::Success),
                "短命进程被误判：{:?}",
                outcome.status
            );
            assert!(outcome.time.is_some(), "正常退出应记录用时");
            // 峰值内存可能一次都没采到（此时为 None），但绝不能因此改判状态
        }
    }

    /// 选择子进程负载的环境变量
    const PAYLOAD_ENV: &str = "TUACK_NG_PROCESS_TEST_PAYLOAD";

    /// 负载用例在测试二进制里的全名，由 [`payload_command`] 精确拉起
    const PAYLOAD_TEST: &str = "process::tests::child_payload";

    /// 构造「把自己再拉起一次，只跑负载用例」的命令
    fn payload_command(kind: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().expect("取不到测试二进制路径"));
        cmd.args(["--exact", PAYLOAD_TEST, "--test-threads=1"]);
        cmd.env(PAYLOAD_ENV, kind);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd
    }

    /// 有界地占住内存：最多 `cap` 字节，逐页触碰保证 RSS 真的增长
    ///
    /// 上限保证即使监控失效，负载也只是占住一小块内存后自行退出，不会拖垮机器。
    fn hold_memory(cap: usize, chunk: usize) {
        let mut held: Vec<Vec<u8>> = Vec::new();
        while held.len() * chunk < cap {
            let mut block = vec![0u8; chunk];
            // vec![0] 走的是惰性零页，必须逐页写入才会计入 RSS
            for page in (0..block.len()).step_by(4096) {
                block[page] = 1;
            }
            held.push(block);
            // 给监控留出采样窗口
            std::thread::sleep(Duration::from_millis(5));
        }
        // 保住内存等待被终止；监控失效时睡完自行退出
        std::thread::sleep(Duration::from_secs(10));
    }

    /// 子进程负载入口：由 TLE/MLE 用例经环境变量拉起；正常跑测试时是空操作
    #[test]
    fn child_payload() {
        let Ok(kind) = std::env::var(PAYLOAD_ENV) else {
            return;
        };
        match kind.as_str() {
            "sleep" => std::thread::sleep(Duration::from_secs(10)),
            // 上限 320 MiB、每块 8 MiB，远低于常见的机器内存
            "alloc" => hold_memory(320 << 20, 8 << 20),
            other => panic!("未知的负载类型：{other}"),
        }
    }

    /// 拉起负载并监督，返回结论与耗时；顺带断言监督返回后子进程已被回收
    fn supervise_payload(kind: &str, limits: ResourceLimits) -> (SuperviseOutcome, Duration) {
        let cmd = payload_command(kind);
        let started = std::time::Instant::now();
        let (outcome, running) = monitor_runtime().block_on(async move {
            let mut child = tokio::process::Command::from(cmd)
                .spawn()
                .expect("启动子进程失败");
            let outcome = ProcessSupervisor::new(limits)
                .supervise(&mut child)
                .await
                .expect("监督子进程失败");
            let running = child.try_wait().expect("查询子进程状态失败").is_none();
            (outcome, running)
        });
        assert!(!running, "监督返回后子进程仍在运行");
        (outcome, started.elapsed())
    }

    /// 超时进程必须被终止并回收，而不是等它自己退出
    #[test]
    fn tle_kills_child() {
        let (outcome, waited) =
            supervise_payload("sleep", limits(Duration::from_millis(200), 1 << 30));
        assert!(
            matches!(&outcome.status, RunStatus::TimeLimitExceeded),
            "期望 TLE，实际 {:?}",
            outcome.status
        );
        let killed_at = outcome.time.expect("超时应记录终止时刻");
        assert!(
            killed_at > Duration::from_millis(200),
            "终止时刻应在限时之后：{killed_at:?}"
        );
        assert!(outcome.memory.is_some(), "TLE 前应已采到峰值内存");
        assert!(
            waited < Duration::from_secs(5),
            "没有及时终止超时进程：{waited:?}"
        );
    }

    /// 内存超限的进程必须被终止并回收，并报告超过上限的真实峰值
    #[test]
    fn mle_kills_child() {
        let memory_limit = 128 << 20;
        let (outcome, waited) =
            supervise_payload("alloc", limits(Duration::from_secs(60), memory_limit));
        assert!(
            matches!(&outcome.status, RunStatus::MemoryLimitExceeded),
            "期望 MLE，实际 {:?}",
            outcome.status
        );
        assert!(
            outcome.memory.is_some_and(|memory| memory > memory_limit),
            "应报告超过上限的真实峰值，实际 {:?}",
            outcome.memory
        );
        assert!(outcome.time.is_some(), "MLE 应记录终止时刻");
        assert!(
            waited < Duration::from_secs(5),
            "没有及时终止超限进程：{waited:?}"
        );
    }
}
