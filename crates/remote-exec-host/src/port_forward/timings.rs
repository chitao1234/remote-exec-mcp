use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PortTunnelTimings {
    pub(crate) resume_timeout: Duration,
}

impl PortTunnelTimings {
    #[cfg(not(test))]
    pub(crate) fn production() -> Self {
        Self {
            resume_timeout: Duration::from_secs(10),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            resume_timeout: Duration::from_millis(100),
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
