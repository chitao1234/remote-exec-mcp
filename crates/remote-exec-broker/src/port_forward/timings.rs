use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PortTunnelTimings {
    pub(crate) heartbeat_interval: Duration,
    pub(crate) heartbeat_timeout: Duration,
}

impl PortTunnelTimings {
    #[cfg(not(test))]
    pub(crate) fn production() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(10),
            heartbeat_timeout: Duration::from_secs(30),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            heartbeat_interval: Duration::from_millis(25),
            heartbeat_timeout: Duration::from_millis(250),
        }
    }
}

pub(crate) fn timings() -> PortTunnelTimings {
    #[cfg(test)]
    {
        PortTunnelTimings::for_test()
    }
    #[cfg(not(test))]
    {
        PortTunnelTimings::production()
    }
}
