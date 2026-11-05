//! T012.4：三家预设（地址照 Magpie presets.go；目录 id 用 models.dev 的）。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub endpoint: &'static str,
    pub catalog: &'static str,
    pub website: &'static str,
    pub keys_url: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "deepseek",
        name: "DeepSeek",
        endpoint: "https://api.deepseek.com/anthropic",
        catalog: "deepseek",
        website: "https://platform.deepseek.com",
        keys_url: "https://platform.deepseek.com/api_keys",
    },
    Preset {
        id: "minimax",
        name: "MiniMax",
        endpoint: "https://api.minimax.io/anthropic",
        catalog: "minimax",
        website: "https://platform.minimax.io",
        keys_url: "https://platform.minimax.io/user-center/basic-information/interface-key",
    },
    Preset {
        id: "minimax-cn",
        name: "MiniMax（国内站）",
        endpoint: "https://api.minimaxi.com/anthropic",
        catalog: "minimax-cn",
        website: "https://platform.minimaxi.com",
        keys_url: "https://platform.minimaxi.com/user-center/basic-information/interface-key",
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

pub fn preset_for_endpoint(endpoint: &str) -> Option<&'static Preset> {
    PRESETS
        .iter()
        .find(|preset| endpoint.starts_with(preset.endpoint))
}
