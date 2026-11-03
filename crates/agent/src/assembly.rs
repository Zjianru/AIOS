//! T006.15：Agent 的唯一产品组装入口（照 DSH bundle/base 的 agent-loop 组合：
//! packages/bundle/base/cordis.patch.yml:516-517 的 agent-loop 行；create/resume 装配与
//! 初始化失败不唤醒的回滚语义照 packages/core/agent-loop/src/index.ts:557,668,717,754 与
//! :601-647）。
//!
//! 产品调用方（CLI chat/run、后续服务 T006.16a）只调 Agent::open：模型、工具集、会话存储
//! 的构造集中在这里，再交接给 create/resume；per-Agent 的 capability 注册与 inbox 恢复仍在
//! driver::Inner::start（单一位置，不重复装配）。

use std::path::PathBuf;

use model::DeepSeek;
use session::{SessionId, SessionStore};
use tools::ToolSet;

use crate::{Agent, AgentConfig, AgentError, Deps};

/// 一次 Agent 组装的全部输入（对应 DSH bundle 里 agent-loop 行 + provider/会话配置）。
///
/// 字段都是调用方已经解析好的值：CLI 的 env/参数、服务配置都在外面变成普通数据，
/// 组装层不读环境变量、不猜默认路径。
#[derive(Clone)]
pub struct AgentOpenOptions {
    /// 会话数据目录（sessions/<id>.jsonl、spill、jobs 都挂在下面）。
    pub data_dir: PathBuf,
    /// 新会话的工作目录（续接时以日志 header 的 cwd 为准）。
    pub cwd: PathBuf,
    /// None = 新建会话；Some(id) = 续接该会话日志（DSH SessionStartSource 的 startup/resume）。
    pub resume: Option<SessionId>,
    /// DeepSeek 路由配置（endpoint/key/model/token 预算/effort）。
    pub model: model::DeepSeekConfig,
    /// Agent 行为配置（并发、压缩、spill、指令/skill 根、todo 策略）。
    pub config: AgentConfig,
    /// T008.2：凭证来源（Keyring 适配器）；None 用 DeepSeekConfig.api_key（测试/旧路径）。
    pub credentials: Option<std::sync::Arc<dyn model::CredentialSource>>,
}

impl std::fmt::Debug for AgentOpenOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentOpenOptions")
            .field("data_dir", &self.data_dir)
            .field("cwd", &self.cwd)
            .field("resume", &self.resume)
            .field("model", &self.model)
            .field("config", &self.config)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "<source>"),
            )
            .finish()
    }
}

impl Agent {
    /// 唯一的 Agent 组装入口：一处构造 DeepSeek + 内置 ToolSet + SessionStore，再交接
    /// create/resume（capability/inbox 在 driver::Inner::start 装配）。
    ///
    /// 失败语义照 DSH：返回 Err 时不留下已启动的驱动任务、不唤醒排队输入、不把
    /// 半装配状态交给调用方（初始化失败由 create/resume 的 Err 承担）。
    pub async fn open(options: AgentOpenOptions) -> Result<Self, AgentError> {
        // 配置在装载时先失败（DSH docs/user/develop/basic/config.md「Fail loudly on invalid
        // configuration」）：不先物化会话文件，也不把半装配状态交给调用方。
        options.config.validate()?;
        let model = match options.credentials {
            Some(credentials) => DeepSeek::with_credentials(options.model, credentials)?,
            None => DeepSeek::new(options.model)?,
        };
        let tools = ToolSet::builtin();
        match options.resume {
            Some(id) => {
                let store = SessionStore::open(&options.data_dir, &id)?;
                Self::resume(
                    id,
                    options.config,
                    Deps {
                        model,
                        tools,
                        store,
                    },
                )
                .await
            }
            None => {
                let store = SessionStore::create(&options.data_dir, &options.cwd)?;
                Self::create(
                    options.config,
                    Deps {
                        model,
                        tools,
                        store,
                    },
                )
                .await
            }
        }
    }
}
