fn uuid_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn new_instance_id() -> String {
    format!("inst_{}", uuid_suffix())
}

pub fn new_exec_session_id() -> String {
    format!("sess_{}", uuid_suffix())
}

pub fn new_tunnel_session_id() -> String {
    format!("ptun_{}", uuid_suffix())
}

pub fn new_public_session_id() -> String {
    format!("sess_{}", uuid_suffix())
}

pub fn new_forward_id() -> String {
    format!("fwd_{}", uuid_suffix())
}
