use std::fmt;

fn uuid_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(prefix: &str) -> Self {
                Self(format!("{prefix}_{}", uuid_suffix()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

id_type!(InstanceId);
id_type!(ExecSessionId);
id_type!(TunnelSessionId);
id_type!(PublicSessionId);
id_type!(ForwardId);

pub fn new_instance_id() -> InstanceId {
    InstanceId::new("inst")
}

pub fn new_exec_session_id() -> ExecSessionId {
    ExecSessionId::new("sess")
}

pub fn new_tunnel_session_id() -> TunnelSessionId {
    TunnelSessionId::new("ptun")
}

pub fn new_public_session_id() -> PublicSessionId {
    PublicSessionId::new("sess")
}

pub fn new_forward_id() -> ForwardId {
    ForwardId::new("fwd")
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_ids_keep_expected_prefixes() {
        assert!(super::new_instance_id().as_str().starts_with("inst_"));
        assert!(super::new_exec_session_id().as_str().starts_with("sess_"));
        assert!(super::new_tunnel_session_id().as_str().starts_with("ptun_"));
        assert!(super::new_public_session_id().as_str().starts_with("sess_"));
        assert!(super::new_forward_id().as_str().starts_with("fwd_"));
    }
}
