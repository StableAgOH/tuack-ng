use std::process::Command as StdCommand;
use std::process::Stdio;
use tempfile::TempDir;

use super::{failed_run, finish_run, inject_input, wire_stdio};
use crate::command::string_to_command;
use crate::prelude::*;
use crate::process::ProcessSupervisor;
use tuack_lib::utils::compiler::{
    RunError, RunResult, RunSpec, Runner, RunnerError, RunnerManifest,
};

/// C++ 运行器的 [`Runner`] 实现：用 `g++` 编译源文件后执行；`set_interactive` 后额外
/// 链接 grader.cpp，并把交互库头文件复制为 `{program_name}.h` 供其包含
pub struct CppRunner {
    tmp_dir: Arc<TempDir>,
    source: PathBuf,
    compile_args: String,
    program_name: String,
    interactive: bool,
    grader_path: Option<PathBuf>,
    header_path: Option<PathBuf>,
}

impl CppRunner {
    /// 创建临时目录，并按源文件扩展名从 `compile_args` 取编译选项。
    ///
    /// # Errors
    ///
    /// 源文件无扩展名，或 `compile_args` 未登记该扩展名时返回 `Err`。
    pub fn new(
        source: impl Into<PathBuf>,
        compile_args: &IndexMap<String, String>,
        program_name: impl Into<String>,
    ) -> Result<Self> {
        let source = source.into();
        let program_name = program_name.into();
        let tmp_dir = Arc::new(TempDir::with_prefix("tuack-ng-runner-")?);
        let ext = source
            .extension()
            .context("没有后缀名")?
            .to_string_lossy()
            .into_owned();
        Ok(CppRunner {
            tmp_dir,
            source,
            compile_args: compile_args
                .get(&ext)
                .context("没有该语言编译选项")?
                .to_string(),
            program_name,
            interactive: false,
            grader_path: None,
            header_path: None,
        })
    }

    fn get_compile_command(&self) -> Result<StdCommand> {
        let exe_path = self
            .tmp_dir
            .path()
            .join(format!(
                "{}{}",
                self.program_name,
                std::env::consts::EXE_SUFFIX
            ))
            .display()
            .to_string()
            .replace("\\", "\\\\")
            .replace(" ", "\\ ");

        let source_path = self
            .tmp_dir
            .path()
            .join(&self.program_name)
            .with_extension(self.source.extension().unwrap())
            .display()
            .to_string()
            .replace("\\", "\\\\")
            .replace(" ", "\\ ");

        let mut cmd_str = format!("g++ -o {} {} {}", exe_path, self.compile_args, source_path);

        if self.interactive {
            let grader_path = self
                .tmp_dir
                .path()
                .join("grader.cpp")
                .display()
                .to_string()
                .replace("\\", "\\\\")
                .replace(" ", "\\ ");
            cmd_str = format!("{} {}", cmd_str, grader_path);
        }

        string_to_command(&cmd_str)
    }
}

impl Runner for CppRunner {
    fn manifest(&self) -> RunnerManifest {
        RunnerManifest { interactive: true }
    }

    fn prepare(&mut self) -> Result<()> {
        if !self.tmp_dir.path().exists() {
            fs::create_dir_all(self.tmp_dir.path())?;
        }

        let mut cmd = self.get_compile_command()?;

        let source_target_path = self
            .tmp_dir
            .path()
            .join(&self.program_name)
            .with_extension(self.source.extension().unwrap());

        fs::copy(&self.source, &source_target_path)?;

        if self.interactive {
            let grader_target_path = self.tmp_dir.path().join("grader.cpp");
            fs::copy(self.grader_path.as_ref().unwrap(), &grader_target_path)?;
            let header_target_path = self.tmp_dir.path().join(format!("{}.h", self.program_name));
            fs::copy(self.header_path.as_ref().unwrap(), &header_target_path)?;
        }

        let output = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
        if !output.status.success() {
            bail!("编译错误：{}", String::from_utf8_lossy(&output.stderr));
        }

        fs::remove_file(&source_target_path)?;

        Ok(())
    }

    fn set_interactive(&mut self, grader_file: &Path, header_file: &Path) -> Result<()> {
        self.interactive = true;
        self.grader_path = Some(grader_file.to_owned());
        self.header_path = Some(header_file.to_owned());
        Ok(())
    }

    fn execute(&mut self, spec: RunSpec) -> Result<RunResult, RunnerError> {
        let RunSpec {
            limits,
            io_mode,
            mut input,
        } = spec;

        let program_path = self.tmp_dir.path().join(format!(
            "{}{}",
            self.program_name,
            std::env::consts::EXE_SUFFIX
        ));

        let mut cmd = StdCommand::new(&program_path);
        cmd.current_dir(self.tmp_dir.path());

        if let Err(error) = inject_input(self.tmp_dir.path(), &io_mode, &mut *input) {
            return Ok(failed_run(RunError::Input(error)));
        }
        if let Err(error) = wire_stdio(self.tmp_dir.path(), &io_mode, &mut cmd) {
            return Ok(failed_run(RunError::Stdio(error)));
        }

        let outcome = ProcessSupervisor::new(limits).supervise_blocking(cmd)?;
        match finish_run(outcome, &self.tmp_dir, &io_mode) {
            Ok(run) => Ok(run),
            Err(error) => Ok(failed_run(RunError::Output(error))),
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        Ok(())
    }
}
