#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerTool {
    ListTargets,
    ExecCommand,
    WriteStdin,
    ApplyPatch,
    ViewImage,
    TransferFiles,
    ForwardPorts,
}

impl BrokerTool {
    #[cfg(test)]
    pub(crate) const NAMES: &'static [&'static str] = &[
        "list_targets",
        "exec_command",
        "write_stdin",
        "apply_patch",
        "view_image",
        "transfer_files",
        "forward_ports",
    ];

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "list_targets" => Some(Self::ListTargets),
            "exec_command" => Some(Self::ExecCommand),
            "write_stdin" => Some(Self::WriteStdin),
            "apply_patch" => Some(Self::ApplyPatch),
            "view_image" => Some(Self::ViewImage),
            "transfer_files" => Some(Self::TransferFiles),
            "forward_ports" => Some(Self::ForwardPorts),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ListTargets => "list_targets",
            Self::ExecCommand => "exec_command",
            Self::WriteStdin => "write_stdin",
            Self::ApplyPatch => "apply_patch",
            Self::ViewImage => "view_image",
            Self::TransferFiles => "transfer_files",
            Self::ForwardPorts => "forward_ports",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BrokerTool;

    #[test]
    fn all_tool_names_round_trip_through_registry() {
        for name in BrokerTool::NAMES {
            let tool = BrokerTool::from_name(name).expect("registered tool should parse");
            assert_eq!(tool.name(), *name);
        }
    }
}

#[cfg(test)]
mod mcp_registration_contract_tests {
    use super::BrokerTool;

    const MCP_TOOL_NAMES: &[&str] = &[
        "list_targets",
        "exec_command",
        "write_stdin",
        "apply_patch",
        "view_image",
        "transfer_files",
        "forward_ports",
    ];

    #[test]
    fn registry_matches_mcp_tool_names() {
        assert_eq!(BrokerTool::NAMES, MCP_TOOL_NAMES);
    }
}
