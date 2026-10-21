//! Tuack-NG 命令行入口：定义子命令并分发到各模块。
//!
//! 退出码约定：运行正常为 0；遇到了用户有关的问题（相关问题已在函数内输出）为 1；
//! 遇到了程序自身的问题为 2，错误由顶层输出。
//!
//! 日志与 panic 策略随构建类型变化：调试构建下详细日志默认开启（`--silent` 关闭），
//! panic 原样抛出；发布构建下详细日志默认关闭（`--verbose` 打开）。

use crate::conf::ConfArgs;
use crate::dmk::DmkArgs;
use crate::doc::DocArgs;
use crate::dump::DumpArgs;
use crate::generate::GenArgs;
use crate::plugin::PluginArgs;
use crate::prelude::*;
use crate::ren::RenArgs;
use crate::test::TestArgs;
use crate::validate::ValidateArgs;
use clap::{Parser, Subcommand};
use clap_i18n_richformatter::clap_i18n;

mod conf;
mod context;
mod develop;
mod dmk;
mod doc;
mod dump;
mod errors;
mod generate;
mod init;
mod plugin;
mod prelude;
mod ren;
mod test;
mod utils;
mod validate;

#[derive(Debug, Parser)]
#[clap_i18n]
#[command(version, about = "Tuack-NG", disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[arg(short, long, global = true)]
    /// 详细模式
    verbose: bool,
    #[cfg(debug_assertions)]
    #[arg(long, global = true)]
    /// 静默模式，无视详细（仅调试构建）
    silent: bool,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = false)]
enum Commands {
    /// 渲染题面
    Ren(RenArgs),
    /// 生成工程文件夹
    Gen(GenArgs),
    /// 使用题解代码测试
    Test(TestArgs),
    /// 批量修改配置文件
    Conf(ConfArgs),
    /// 生成数据
    Dmk(DmkArgs),
    /// 校验输入数据
    Validate(ValidateArgs),
    /// 导出到评测系统
    Dump(DumpArgs),
    /// 文档相关
    Doc(DocArgs),
    /// 开发工具
    Develop(develop::DevelopArgs),
    /// 插件管理
    Plugin(PluginArgs),
}

fn tuack_ng(cli: Cli) -> Result<()> {
    init::init(
        &{
            #[cfg(debug_assertions)]
            {
                !cli.silent
            }
            #[cfg(not(debug_assertions))]
            {
                cli.verbose
            }
        },
        &cli,
    )?;
    info!("booting up");

    match cli.command {
        Commands::Ren(args) => ren::main(args),
        Commands::Gen(args) => generate::main(args),
        Commands::Test(args) => test::main(args),
        Commands::Conf(args) => conf::main(args),
        Commands::Dmk(args) => dmk::main(args),
        Commands::Validate(args) => validate::main(args),
        Commands::Dump(args) => dump::main(args),
        Commands::Doc(args) => doc::main(args),
        Commands::Develop(args) => develop::main(args),
        Commands::Plugin(args) => plugin::main(args),
    }
}

/// 把子命令结果与错误统一落成退出码：正常 0、用户问题 1、自身问题 2。
///
/// 输出分工：用户问题（`Reported`）已由抛出它的代码输出，顶层不重复；其余错误都是未处理的
/// 自身问题（Bug/安装，或某处用户问题忘了就地报告），由顶层输出并以 2 退出。
fn exit_code(result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        // 用户问题已在抛出点输出
        Err(error) if is_reported(&error) => 1,
        // 未报告的错误 = 未处理的自身问题
        Err(error) => {
            report(&error);
            2
        }
    }
}

/// 输出未处理的自身问题
fn report(error: &anyhow::Error) {
    if cfg!(debug_assertions) {
        msg_error!("程序执行出错：{:?}", error);
    } else {
        msg_error!("程序执行出错：{:#}", error);
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse_i18n_or_exit();

    let code = exit_code(tuack_ng(cli));
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
