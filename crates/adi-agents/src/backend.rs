#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    TmuxClaude,
    TmuxCodex,
    ProcessClaude,
    ProcessCodex,
    Wasm,
}

impl Backend {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "tmux:claude" => Some(Self::TmuxClaude),
            "tmux:codex" => Some(Self::TmuxCodex),
            "process:claude" => Some(Self::ProcessClaude),
            "process:codex" => Some(Self::ProcessCodex),
            "wasm:loop-script" => Some(Self::Wasm),
            _ => None,
        }
    }

    pub(crate) const fn executor(self) -> &'static str {
        match self {
            Self::TmuxClaude | Self::TmuxCodex => "tmux",
            Self::ProcessClaude | Self::ProcessCodex => "process",
            Self::Wasm => "wasm",
        }
    }
}
