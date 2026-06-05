#[derive(Clone, Copy)]
pub(in crate::daemon_client) enum RpcLogSubject<'a> {
    Path(&'a str),
    DestinationPath(&'a str),
}

#[derive(Clone, Copy)]
pub(in crate::daemon_client) enum RpcCallKind {
    Rpc,
    TransferExport,
    TransferImport,
}

#[derive(Clone, Copy)]
pub(in crate::daemon_client) struct RpcCallContext<'a> {
    target_name: &'a str,
    base_url: &'a str,
    started: std::time::Instant,
    kind: RpcCallKind,
    subject: RpcLogSubject<'a>,
}

impl RpcCallKind {
    fn label(self, suffix: &str) -> String {
        let prefix = match self {
            Self::Rpc => "daemon rpc",
            Self::TransferExport => "daemon transfer export",
            Self::TransferImport => "daemon transfer import",
        };
        format!("{prefix} {suffix}")
    }
}

macro_rules! log_rpc {
    ($level:ident, $ctx:expr, $msg:expr $(, $($field:tt)*)?) => {{
        let elapsed_ms = $ctx.started.elapsed().as_millis() as u64;
        match $ctx.subject {
            RpcLogSubject::Path(path) => tracing::$level!(
                target = %$ctx.target_name,
                base_url = %$ctx.base_url,
                path,
                elapsed_ms,
                $($($field)*)?
                "{}", $msg
            ),
            RpcLogSubject::DestinationPath(destination_path) => tracing::$level!(
                target = %$ctx.target_name,
                base_url = %$ctx.base_url,
                destination_path,
                elapsed_ms,
                $($($field)*)?
                "{}", $msg
            ),
        }
    }};
}

impl<'a> RpcCallContext<'a> {
    pub(in crate::daemon_client) fn path(
        target_name: &'a str,
        base_url: &'a str,
        started: std::time::Instant,
        kind: RpcCallKind,
        path: &'a str,
    ) -> Self {
        Self {
            target_name,
            base_url,
            started,
            kind,
            subject: RpcLogSubject::Path(path),
        }
    }

    pub(in crate::daemon_client) fn destination_path(
        target_name: &'a str,
        base_url: &'a str,
        started: std::time::Instant,
        kind: RpcCallKind,
        destination_path: &'a str,
    ) -> Self {
        Self {
            target_name,
            base_url,
            started,
            kind,
            subject: RpcLogSubject::DestinationPath(destination_path),
        }
    }

    pub(in crate::daemon_client) fn log_completed(self) {
        log_rpc!(debug, self, self.kind.label("completed"));
    }

    pub(in crate::daemon_client) fn log_transport_error(self, err: &reqwest::Error) {
        if self.is_health_rpc() {
            log_rpc!(debug, self, self.kind.label("transport failed"), error = %err,);
        } else {
            log_rpc!(warn, self, self.kind.label("transport failed"), error = %err,);
        }
    }

    pub(in crate::daemon_client) fn log_status_error(self, status: reqwest::StatusCode) {
        if self.is_health_rpc() {
            log_rpc!(
                debug,
                self,
                self.kind.label("returned error status"),
                status = status.as_u16(),
            );
        } else {
            log_rpc!(
                warn,
                self,
                self.kind.label("returned error status"),
                status = status.as_u16(),
            );
        }
    }

    pub(in crate::daemon_client) fn log_read_error(self, err: &reqwest::Error) {
        if self.is_health_rpc() {
            log_rpc!(debug, self, self.kind.label("body read failed"), error = %err,);
        } else {
            log_rpc!(warn, self, self.kind.label("body read failed"), error = %err,);
        }
    }

    pub(in crate::daemon_client) fn log_decode_error(self, err: &serde_json::Error) {
        if self.is_health_rpc() {
            log_rpc!(debug, self, self.kind.label("decode failed"), error = %err,);
        } else {
            log_rpc!(warn, self, self.kind.label("decode failed"), error = %err,);
        }
    }

    pub(in crate::daemon_client) fn is_health_rpc(self) -> bool {
        matches!(self.subject, RpcLogSubject::Path("/v1/health"))
    }
}
