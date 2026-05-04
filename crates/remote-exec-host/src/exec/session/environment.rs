use portable_pty::CommandBuilder;

use crate::config::ProcessEnvironment;

const NORMALIZED_ENV: [(&str, &str); 7] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("CODEX_CI", "1"),
];

pub(super) fn normalized_pairs(environment: &ProcessEnvironment) -> Vec<(String, String)> {
    let mut pairs = NORMALIZED_ENV
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    pairs.extend(super::super::locale::LocaleEnvPlan::resolved(environment).as_pairs());
    pairs
}

pub(super) fn apply_overlay_builder(
    builder: &mut CommandBuilder,
    environment: &ProcessEnvironment,
) {
    apply_base_environment_builder(builder, environment);
    builder.env_remove("LANG");
    builder.env_remove("LC_CTYPE");
    builder.env_remove("LC_ALL");
    for (key, value) in normalized_pairs(environment) {
        builder.env(&key, &value);
    }
}

pub(super) fn apply_overlay_std_command(
    command: &mut std::process::Command,
    environment: &ProcessEnvironment,
) {
    apply_base_environment_std_command(command, environment);
    command.env_remove("LANG");
    command.env_remove("LC_CTYPE");
    command.env_remove("LC_ALL");
    for (key, value) in normalized_pairs(environment) {
        command.env(&key, &value);
    }
}

fn apply_base_environment_builder(builder: &mut CommandBuilder, environment: &ProcessEnvironment) {
    builder.env_clear();
    for (key, value) in environment.vars() {
        builder.env(key, value);
    }
}

fn apply_base_environment_std_command(
    command: &mut std::process::Command,
    environment: &ProcessEnvironment,
) {
    command.env_clear();
    for (key, value) in environment.vars() {
        command.env(key, value);
    }
}
