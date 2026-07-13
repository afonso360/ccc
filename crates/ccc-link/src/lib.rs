//! Target tool resolution and executable link-plan execution.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use ccc_target::{EffectiveCompilationConfig, RelocationModel, Triple};

#[derive(Debug)]
pub struct LinkError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for LinkError {}

pub fn link_executable(
    object: &Path,
    output: &Path,
    config: &EffectiveCompilationConfig,
) -> Result<(), LinkError> {
    let driver = resolve_driver(config)?;
    let mut command = driver.command();
    command.arg(object).arg("-o").arg(output);
    match config.relocation_model {
        RelocationModel::Static => {
            command.arg("-no-pie");
        }
    }
    let result = command.output().map_err(|error| LinkError {
        code: "CCC5003",
        message: format!(
            "cannot invoke target compiler driver `{}`: {error}",
            driver.display()
        ),
    })?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(LinkError {
            code: "CCC5004",
            message: format!(
                "target compiler driver `{}` failed: {}",
                driver.display(),
                stderr.trim()
            ),
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ToolCommand {
    program: PathBuf,
    arguments: Vec<OsString>,
}

impl ToolCommand {
    fn from_environment(value: OsString) -> Result<Self, LinkError> {
        let value = value.to_string_lossy();
        let mut words = value.split_whitespace();
        let program = words.next().ok_or_else(|| LinkError {
            code: "CCC5002",
            message: "target compiler driver environment entry is empty".to_owned(),
        })?;
        Ok(Self {
            program: PathBuf::from(program),
            arguments: words.map(OsString::from).collect(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.arguments);
        command
    }

    fn display(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.arguments
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn resolve_driver(config: &EffectiveCompilationConfig) -> Result<ToolCommand, LinkError> {
    let candidate = env::var_os("CCC_CC")
        .or_else(|| env::var_os("CC"))
        .map_or_else(
            || {
                Ok(ToolCommand {
                    program: PathBuf::from("cc"),
                    arguments: Vec::new(),
                })
            },
            ToolCommand::from_environment,
        )?;
    let mut failures = Vec::new();
    match reported_target(&candidate) {
        Ok(reported) if target_matches(&reported, &config.target.triple) => {
            return Ok(candidate);
        }
        Ok(reported) => failures.push(format!(
            "`{}` reports target `{reported}`",
            candidate.display()
        )),
        Err(error) => failures.push(format!("`{}`: {error}", candidate.display())),
    }
    Err(LinkError {
        code: "CCC5005",
        message: format!(
            "no compiler driver for target `{}` was found ({})",
            config.target.triple,
            failures.join("; ")
        ),
    })
}

fn reported_target(driver: &ToolCommand) -> Result<Triple, String> {
    let result = driver
        .command()
        .arg("-dumpmachine")
        .output()
        .map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(format!("target probe exited with {}", result.status));
    }
    let target = String::from_utf8(result.stdout).map_err(|error| error.to_string())?;
    let target = target.trim();
    if target.is_empty() {
        return Err("target probe returned an empty target".to_owned());
    }
    target
        .parse()
        .map_err(|error| format!("target probe returned invalid target `{target}`: {error}"))
}

fn target_matches(reported: &Triple, expected: &Triple) -> bool {
    reported.architecture == expected.architecture
        && reported.operating_system == expected.operating_system
        && reported.environment == expected.environment
        && reported.binary_format == expected.binary_format
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_common_spellings_of_the_primary_target() {
        let expected: Triple = "x86_64-unknown-linux-gnu".parse().unwrap();
        assert!(target_matches(
            &"x86_64-linux-gnu".parse().unwrap(),
            &expected
        ));
        assert!(target_matches(
            &"x86_64-redhat-linux-gnu".parse().unwrap(),
            &expected
        ));
        assert!(!target_matches(
            &"x86_64-apple-darwin".parse().unwrap(),
            &expected
        ));
    }

    #[test]
    fn splits_a_compiler_driver_from_its_leading_arguments() {
        let command = ToolCommand::from_environment(OsString::from("ccache cc -m64")).unwrap();
        assert_eq!(command.program, PathBuf::from("ccache"));
        assert_eq!(
            command.arguments,
            [OsString::from("cc"), OsString::from("-m64")]
        );
    }
}
