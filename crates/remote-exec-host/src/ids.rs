use remote_exec_proto::port_forward::ForwardId;

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

pub fn new_instance_id() -> String {
    new_id("inst")
}

pub fn new_exec_session_id() -> String {
    new_id("sess")
}

pub fn new_tunnel_session_id() -> String {
    new_id("ptun")
}

pub fn new_public_session_id() -> String {
    new_id("sess")
}

pub fn new_forward_id() -> ForwardId {
    ForwardId::new(new_id("fwd"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_ids_keep_expected_prefixes() {
        assert!(super::new_instance_id().starts_with("inst_"));
        assert!(super::new_exec_session_id().starts_with("sess_"));
        assert!(super::new_tunnel_session_id().starts_with("ptun_"));
        assert!(super::new_public_session_id().starts_with("sess_"));
        assert!(super::new_forward_id().as_str().starts_with("fwd_"));
    }
}
