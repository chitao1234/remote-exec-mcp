use std::ffi::{OsStr, OsString};

#[derive(Debug, Clone, Default)]
pub struct ProcessEnvironment {
    vars: Vec<(OsString, OsString)>,
}

impl ProcessEnvironment {
    pub fn capture_current() -> Self {
        Self {
            vars: std::env::vars_os().collect(),
        }
    }

    pub fn path(&self) -> Option<&OsStr> {
        self.var_os("PATH")
    }

    pub fn comspec(&self) -> Option<&str> {
        self.var_os("COMSPEC").and_then(|value| value.to_str())
    }

    pub fn vars(&self) -> &[(OsString, OsString)] {
        &self.vars
    }

    pub fn var_os(&self, key: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .find(|(existing_key, _)| env_key_matches(existing_key, key))
            .map(|(_, value)| value.as_os_str())
    }

    pub fn set_var(&mut self, key: &str, value: Option<OsString>) {
        self.vars
            .retain(|(existing_key, _)| !env_key_matches(existing_key, key));

        if let Some(value) = value {
            self.vars.push((OsString::from(key), value));
        }
    }
}

fn env_key_matches(existing_key: &OsStr, requested_key: &str) -> bool {
    #[cfg(windows)]
    {
        existing_key
            .to_string_lossy()
            .eq_ignore_ascii_case(requested_key)
    }

    #[cfg(not(windows))]
    {
        existing_key == OsStr::new(requested_key)
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessEnvironment;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn path_and_comspec_are_derived_from_vars() {
        let mut environment = ProcessEnvironment::default();
        environment.set_var("PATH", Some(OsString::from("/custom/bin")));
        environment.set_var("COMSPEC", Some(OsString::from("cmd.exe")));

        assert_eq!(environment.path(), Some(OsStr::new("/custom/bin")));
        assert_eq!(environment.comspec(), Some("cmd.exe"));

        environment.set_var("PATH", None);
        environment.set_var("COMSPEC", None);

        assert_eq!(environment.path(), None);
        assert_eq!(environment.comspec(), None);
    }
}
