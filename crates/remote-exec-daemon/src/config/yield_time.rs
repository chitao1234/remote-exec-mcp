use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YieldTimeOperation {
    ExecCommand,
    WriteStdinPoll,
    WriteStdinInput,
}

impl YieldTimeOperation {
    fn config_path(self) -> &'static str {
        match self {
            Self::ExecCommand => "yield_time.exec_command",
            Self::WriteStdinPoll => "yield_time.write_stdin_poll",
            Self::WriteStdinInput => "yield_time.write_stdin_input",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YieldTimeOperationConfig {
    pub default_ms: u64,
    pub max_ms: u64,
    pub min_ms: u64,
}

impl YieldTimeOperationConfig {
    pub const fn new(default_ms: u64, max_ms: u64, min_ms: u64) -> Self {
        Self {
            default_ms,
            max_ms,
            min_ms,
        }
    }

    pub fn resolve_ms(self, requested_ms: Option<u64>) -> u64 {
        requested_ms
            .unwrap_or(self.default_ms)
            .clamp(self.min_ms, self.max_ms)
    }

    fn validate(self, operation: YieldTimeOperation) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.min_ms <= self.max_ms,
            "{}.min_ms must be less than or equal to {}.max_ms",
            operation.config_path(),
            operation.config_path()
        );
        anyhow::ensure!(
            self.default_ms >= self.min_ms && self.default_ms <= self.max_ms,
            "{}.default_ms must be between {}.min_ms and {}.max_ms",
            operation.config_path(),
            operation.config_path(),
            operation.config_path()
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YieldTimeConfig {
    pub exec_command: YieldTimeOperationConfig,
    pub write_stdin_poll: YieldTimeOperationConfig,
    pub write_stdin_input: YieldTimeOperationConfig,
}

impl YieldTimeConfig {
    pub fn resolve_ms(self, operation: YieldTimeOperation, requested_ms: Option<u64>) -> u64 {
        self.operation_config(operation).resolve_ms(requested_ms)
    }

    fn operation_config(self, operation: YieldTimeOperation) -> YieldTimeOperationConfig {
        match operation {
            YieldTimeOperation::ExecCommand => self.exec_command,
            YieldTimeOperation::WriteStdinPoll => self.write_stdin_poll,
            YieldTimeOperation::WriteStdinInput => self.write_stdin_input,
        }
    }

    pub(super) fn validate(self) -> anyhow::Result<()> {
        self.exec_command
            .validate(YieldTimeOperation::ExecCommand)?;
        self.write_stdin_poll
            .validate(YieldTimeOperation::WriteStdinPoll)?;
        self.write_stdin_input
            .validate(YieldTimeOperation::WriteStdinInput)?;
        Ok(())
    }
}

impl Default for YieldTimeConfig {
    fn default() -> Self {
        Self {
            exec_command: default_exec_command_yield_time(),
            write_stdin_poll: default_write_stdin_poll_yield_time(),
            write_stdin_input: default_write_stdin_input_yield_time(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
struct YieldTimeConfigOverride {
    #[serde(default)]
    exec_command: YieldTimeOperationConfigOverride,
    #[serde(default)]
    write_stdin_poll: YieldTimeOperationConfigOverride,
    #[serde(default)]
    write_stdin_input: YieldTimeOperationConfigOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
struct YieldTimeOperationConfigOverride {
    #[serde(default)]
    default_ms: Option<u64>,
    #[serde(default)]
    max_ms: Option<u64>,
    #[serde(default)]
    min_ms: Option<u64>,
}

impl YieldTimeConfigOverride {
    fn resolve(self) -> YieldTimeConfig {
        YieldTimeConfig {
            exec_command: self.exec_command.resolve(default_exec_command_yield_time()),
            write_stdin_poll: self
                .write_stdin_poll
                .resolve(default_write_stdin_poll_yield_time()),
            write_stdin_input: self
                .write_stdin_input
                .resolve(default_write_stdin_input_yield_time()),
        }
    }
}

impl YieldTimeOperationConfigOverride {
    fn resolve(self, defaults: YieldTimeOperationConfig) -> YieldTimeOperationConfig {
        YieldTimeOperationConfig {
            default_ms: self.default_ms.unwrap_or(defaults.default_ms),
            max_ms: self.max_ms.unwrap_or(defaults.max_ms),
            min_ms: self.min_ms.unwrap_or(defaults.min_ms),
        }
    }
}

impl<'de> Deserialize<'de> for YieldTimeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(YieldTimeConfigOverride::deserialize(deserializer)?.resolve())
    }
}

const fn default_exec_command_yield_time() -> YieldTimeOperationConfig {
    YieldTimeOperationConfig::new(10_000, 30_000, 250)
}

const fn default_write_stdin_poll_yield_time() -> YieldTimeOperationConfig {
    YieldTimeOperationConfig::new(5_000, 300_000, 5_000)
}

const fn default_write_stdin_input_yield_time() -> YieldTimeOperationConfig {
    YieldTimeOperationConfig::new(250, 30_000, 250)
}
