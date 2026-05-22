use remote_exec_proto::public::TransferEndpoint;

pub(crate) const TARGET_NAME: &str = "local";

pub(crate) fn is_target_name(name: &str) -> bool {
    name == TARGET_NAME
}

/// A broker-host endpoint is selected with the reserved name `local`, but that
/// does not always mean the configured `[local]` target is enabled.
///
/// Exec, patch, and image operations resolve only configured targets. Transfer
/// and port forwarding may use the broker host directly even when `[local]` is
/// omitted, with their own filesystem/network semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerHostOrTarget<'a> {
    BrokerHost,
    Target(&'a str),
}

impl<'a> BrokerHostOrTarget<'a> {
    pub(crate) fn from_name(name: &'a str) -> Self {
        if is_target_name(name) {
            Self::BrokerHost
        } else {
            Self::Target(name)
        }
    }

    pub(crate) fn from_transfer_endpoint(endpoint: &'a TransferEndpoint) -> Self {
        Self::from_name(&endpoint.target)
    }
}
