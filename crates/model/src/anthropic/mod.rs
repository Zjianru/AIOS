//! Anthropic Messages 适配器：HTTP 传输、SSE 解码和流事件翻译。

mod config;
mod learned;
mod serialize;
mod sse;
mod translate;
mod transport;
mod vendors;

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

pub use config::{DEFAULT_BASE_URL, DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_TOKENS, DeepSeekConfig};
pub use serialize::file_handle_text;
/// F2：这一轮实际会发的档（内核规则；页面不另抄表）。
pub use vendors::actual_effort;

use crate::Effort;
use crate::{
    CancelToken, CredentialContext, CredentialSource, ErrorCode, Message, ModelError, StreamChunk,
    ToolRequest, ToolUpdate,
};
use learned::{AuthChoice, Learned};
use sse::SseParser;
use translate::Translator;

use config::STREAM_IDLE_TIMEOUT;

/// 对话中途加工具（`tool_addition`）要求的 beta 头（照 DSH `deepseek-messages-api.ts:7`）。
pub const MESSAGES_TOOL_CHANGES_BETA: &str = "mid-conversation-tool-changes-2026-07-01";

/// 模型自己的默认档：先按连接的真实厂商 id 查目录，查不到再用地址/模型名的内置表。
/// 新连接没有用户选择时用它（不是写死 high）。
pub fn default_effort_for(provider: &str, base_url: &str, model: &str) -> String {
    if let Some((_thinks, efforts)) = crate::catalog::global()
        .as_ref()
        .and_then(|catalog| catalog.reasoning_for(provider, model))
    {
        return crate::default_effort(&efforts);
    }
    crate::default_effort(&vendors::reasoning(base_url, model).efforts)
}

/// DeepSeek 的 Messages 适配器。
#[derive(Clone)]
pub struct DeepSeek {
    http: reqwest::Client,
    config: DeepSeekConfig,
    url: reqwest::Url,
    credentials: Arc<dyn CredentialSource>,
    issue_context: Arc<Mutex<CredentialContext>>,
    /// 下一轮开始时要用的模型（set_model 只写这里）。
    next_model: Arc<Mutex<Option<String>>>,
    /// 本轮冻结的模型；begin_turn 时从 next_model 复制，stream 只读这里。
    turn_model: Arc<Mutex<Option<String>>>,
    /// 下一轮要用的思考档（set_effort 只写这里）。
    next_effort: Arc<Mutex<Option<String>>>,
    /// 本轮冻结的档；begin_turn 时从 next_effort 复制，stream 只读这里。
    turn_effort: Arc<Mutex<Option<String>>>,
    /// 下一轮要用的上下文窗口（换模型时写这里）。
    next_context_window: Arc<Mutex<Option<u64>>>,
    /// 本轮冻结的上下文窗口；begin_turn 时从 next_context_window 复制。
    turn_context_window: Arc<Mutex<Option<u64>>>,
    /// 学到的连接事实（关不掉思考、通过的鉴权头），同实例的 clone 共享。
    learned: Arc<Learned>,
}

impl fmt::Debug for DeepSeek {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeepSeek")
            .field("config", &self.config)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl DeepSeek {
    /// 用外部凭证来源构造（每次出站前申请一次；产品由 Keyring 适配器提供）。
    pub fn with_credentials(
        config: DeepSeekConfig,
        credentials: Arc<dyn CredentialSource>,
    ) -> Result<Self, ModelError> {
        let url = messages_url(&config.base_url)?;
        // 出站 client 只在 model::http 一处造。
        let http = crate::http::client()?;
        Ok(Self {
            http,
            config,
            url,
            credentials,
            issue_context: Arc::new(Mutex::new(CredentialContext::default())),
            next_model: Arc::new(Mutex::new(None)),
            turn_model: Arc::new(Mutex::new(None)),
            next_effort: Arc::new(Mutex::new(None)),
            turn_effort: Arc::new(Mutex::new(None)),
            next_context_window: Arc::new(Mutex::new(None)),
            turn_context_window: Arc::new(Mutex::new(None)),
            learned: Arc::new(Learned::default()),
        })
    }

    /// 这个模型的 DefaultEffort（压缩摘要用）。
    pub fn default_effort(&self) -> Effort {
        Effort::new(default_effort_for(
            &self.config.provider,
            &self.config.base_url,
            &self.config.model,
        ))
    }

    /// 同一连接/模型换一个要的档（压缩摘要用；不改原实例）。
    /// 克隆不继承运行时冻结的档（压缩摘要用 config.effort）。
    pub fn with_effort(&self, effort: Effort) -> Self {
        let mut clone = self.clone();
        clone.config.effort = effort;
        clone.next_effort = Arc::new(Mutex::new(None));
        clone.turn_effort = Arc::new(Mutex::new(None));
        clone
    }

    /// 这一轮实际发的档和用户要的档（日志用；与实际请求一致）。
    ///
    /// P2：本轮模型以 begin_turn 冻结的为准（与请求体一致）；学到「关不掉思考」后，
    /// 要 none 时实际发最低非关档（与 stream 的重发一致），日志不能记旧 actual。
    pub fn effort_state(&self) -> (String, String) {
        let model = self
            .turn_model
            .lock()
            .ok()
            .and_then(|current| current.clone())
            .unwrap_or_else(|| self.config.model.clone());
        // 本轮要的档以 begin_turn 冻结的为准（中途换档不影响本轮日志）。
        let wanted = self
            .turn_effort
            .lock()
            .ok()
            .and_then(|current| current.clone())
            .unwrap_or_else(|| self.config.effort.as_str().to_owned());
        let effort = Effort::new(wanted.clone());
        let actual = if effort.is_off()
            && self
                .learned
                .thinking_required(&self.config.base_url, &model)
        {
            vendors::lowest_non_off_effort(&self.config.base_url, &model)
        } else {
            vendors::actual_effort(&self.config.base_url, &model, &effort)
        };
        (wanted, actual)
    }

    pub fn config(&self) -> &DeepSeekConfig {
        &self.config
    }

    /// 标记本次请求的归属（run/turn）；授权来源在 issue 时读取。
    pub fn set_issue_context(&self, context: CredentialContext) {
        if let Ok(mut current) = self.issue_context.lock() {
            *current = context;
        }
    }

    /// 本轮冻结的模型（begin_turn 之后的实际请求模型；没有冻结时用 config）。
    pub fn current_model(&self) -> String {
        self.turn_model
            .lock()
            .ok()
            .and_then(|current| current.clone())
            .unwrap_or_else(|| self.config.model.clone())
    }

    /// 本轮开始后冻结的模型；还没开始一轮、或上一轮已经收尾时为 None。
    ///
    /// 只读，不推进状态；服务用它判断「正在跑的一轮」到底在用哪个模型。
    pub fn frozen_model(&self) -> Option<String> {
        self.turn_model
            .lock()
            .ok()
            .and_then(|current| current.clone())
    }

    /// 一轮收尾：清掉冻结的模型，服务不再把刚停下的那一轮当成在跑。
    pub fn end_turn(&self) {
        if let Ok(mut current) = self.turn_model.lock() {
            *current = None;
        }
    }

    /// 换模型：只改下一轮；本轮冻结的模型在 begin_turn 时确定（D26）。
    pub fn set_model(&self, model: String) {
        if let Ok(mut current) = self.next_model.lock() {
            *current = Some(model);
        }
    }

    /// 换思考档：只改下一轮；本轮冻结的档在 begin_turn 时确定（D26）。
    pub fn set_effort(&self, effort: Effort) {
        if let Ok(mut current) = self.next_effort.lock() {
            *current = Some(effort.as_str().to_owned());
        }
    }

    /// 换模型的上下文窗口：只改下一轮；本轮冻结的窗口在 begin_turn 时确定。
    ///
    /// 压缩阈值按本轮实际请求的模型算，换到窗口更小的模型后不能继续按旧窗口估。
    pub fn set_context_window(&self, window: u64) {
        if let Ok(mut current) = self.next_context_window.lock() {
            *current = Some(window);
        }
    }

    /// 这一轮实际使用的上下文窗口：本轮冻结优先，否则用装配时的窗口。
    pub fn context_window(&self) -> u64 {
        self.turn_context_window
            .lock()
            .ok()
            .and_then(|current| *current)
            .unwrap_or(self.config.context_window)
    }

    /// 本轮开始时冻结模型和档；stream 内不再读 next_*（D26）。
    pub fn begin_turn(&self) {
        // 没有待切换的模型时冻结装配时的模型：本轮的实际请求模型就是它。
        let next = self
            .next_model
            .lock()
            .map(|model| model.clone())
            .unwrap_or(None)
            .unwrap_or_else(|| self.config.model.clone());
        if let Ok(mut current) = self.turn_model.lock() {
            *current = Some(next);
        }
        let next = self
            .next_effort
            .lock()
            .map(|effort| effort.clone())
            .unwrap_or(None);
        if let Ok(mut current) = self.turn_effort.lock() {
            *current = next;
        }
        let next = self
            .next_context_window
            .lock()
            .map(|window| *window)
            .unwrap_or(None);
        if let Ok(mut current) = self.turn_context_window.lock() {
            *current = next;
        }
    }

    /// 厂商路由 id（DSH 的 provider 名）：日志、统计和压缩策略都按它匹配。
    pub fn provider(&self) -> &str {
        &self.config.provider
    }

    /// 发一次流式请求。消息和工具定义按 Messages 协议序列化（system 单独成字段）。
    /// 取消令牌能在「等响应头」阶段打断请求（对应 DSH 把 AbortSignal 传进 LLM 层）。
    pub async fn stream(
        &self,
        messages: &[Message],
        tools: &ToolRequest,
        cancel: &CancelToken,
    ) -> Result<ChunkStream, ModelError> {
        self.stream_inner(messages, tools, cancel, None, true).await
    }

    /// 同 `stream`，但这一次请求覆盖输出上限（摘要请求的 maxTokens 上限；DSH GenerateOptions.maxTokens）。
    ///
    /// 摘要请求的输入是**被截断的历史前缀**（压缩事务要摘的就是这段前缀），
    /// 里面可能留下没有配对的 tool_use/tool_result；DSH 的三类防御校验针对完整对话请求，
    /// 不能套在摘要前缀上（否则合法的截断前缀会被误拒）。所以这里跳过配对防御校验。
    pub async fn stream_with_max_tokens(
        &self,
        messages: &[Message],
        tools: &ToolRequest,
        cancel: &CancelToken,
        max_tokens: Option<u32>,
    ) -> Result<ChunkStream, ModelError> {
        self.stream_inner(messages, tools, cancel, max_tokens, false)
            .await
    }

    async fn stream_inner(
        &self,
        messages: &[Message],
        tools: &ToolRequest,
        cancel: &CancelToken,
        max_tokens: Option<u32>,
        validate_pairing: bool,
    ) -> Result<ChunkStream, ModelError> {
        let context = self
            .issue_context
            .lock()
            .map(|context| context.clone())
            .unwrap_or_default();
        let mut effective = self.config.clone();
        if let Ok(current) = self.turn_model.lock()
            && let Some(model) = current.as_deref()
        {
            effective.model = model.to_owned();
        }
        // 本轮冻结的档（用户中途换档不影响正在跑的轮）。
        if let Ok(current) = self.turn_effort.lock()
            && let Some(effort) = current.as_deref()
        {
            effective.effort = crate::Effort::new(effort);
        }
        // 出站之前先查屏蔽：被拒就不签发、不发请求（聊天、重试、摘要共用这一处）。
        self.credentials.admit(&effective.model)?;
        let api_key = self.credentials.issue("model-request", &context)?;
        let base_url = effective.base_url.clone();
        // 学到的「关不掉思考」：要 none 也按最低非关档发（与内置 always_thinks 同行为）。
        if self.learned.thinking_required(&base_url, &effective.model) && effective.effort.is_off()
        {
            effective.effort =
                crate::Effort::new(vendors::lowest_non_off_effort(&base_url, &effective.model));
        }
        let choice = self.auth_choice(&base_url);
        let tool_update = self.route_tool_update(&effective.model);
        let prepared = serialize::request_body(
            &effective,
            messages,
            tools,
            tool_update,
            max_tokens,
            validate_pairing,
        )?;
        let attempt = self
            .post_messages(
                prepared.body,
                prepared.tool_changes,
                &api_key,
                choice,
                cancel,
            )
            .await?;
        if let Some(response) = attempt.response {
            return Ok(ChunkStream::new(response, cancel.clone()));
        }
        // -A：400 拒绝关掉思考 → 学到 + 本轮立即按最低非关档重发一次。
        if attempt.status == reqwest::StatusCode::BAD_REQUEST
            && transport::refuses_thinking_off(&attempt.raw)
        {
            self.learned
                .learn_thinking_required(&base_url, &effective.model);
            effective.effort =
                crate::Effort::new(vendors::lowest_non_off_effort(&base_url, &effective.model));
            let prepared = serialize::request_body(
                &effective,
                messages,
                tools,
                tool_update,
                max_tokens,
                validate_pairing,
            )?;
            let retry = self
                .post_messages(
                    prepared.body,
                    prepared.tool_changes,
                    &api_key,
                    choice,
                    cancel,
                )
                .await?;
            return match retry.response {
                Some(response) => Ok(ChunkStream::new(response, cancel.clone())),
                None => Err(transport::provider_error(
                    &retry.raw,
                    Some(retry.status.as_u16()),
                    Some(&retry.headers),
                )),
            };
        }
        // -B：401 拒绝双头 → 各试一次单头（x-api-key 先、Authorization 后），记住通过的。
        if attempt.status == reqwest::StatusCode::UNAUTHORIZED
            && choice == AuthChoice::Both
            && transport::both_auth_refused(401, &attempt.raw)
        {
            let mut last = (attempt.raw, attempt.headers, 401_u16);
            for only in [AuthChoice::XApiKey, AuthChoice::Authorization] {
                let prepared = serialize::request_body(
                    &effective,
                    messages,
                    tools,
                    tool_update,
                    max_tokens,
                    validate_pairing,
                )?;
                let retry = self
                    .post_messages(prepared.body, prepared.tool_changes, &api_key, only, cancel)
                    .await?;
                if let Some(response) = retry.response {
                    if let Some(name) = only.learned_name() {
                        self.learned.learn_auth_header(&base_url, name);
                        self.credentials.remember_auth_header(name);
                    }
                    return Ok(ChunkStream::new(response, cancel.clone()));
                }
                if retry.status != reqwest::StatusCode::UNAUTHORIZED
                    && retry.status != reqwest::StatusCode::FORBIDDEN
                {
                    return Err(transport::provider_error(
                        &retry.raw,
                        Some(retry.status.as_u16()),
                        Some(&retry.headers),
                    ));
                }
                last = (retry.raw, retry.headers, retry.status.as_u16());
            }
            return Err(transport::provider_error(
                &last.0,
                Some(last.2),
                Some(&last.1),
            ));
        }
        Err(transport::provider_error(
            &attempt.raw,
            Some(attempt.status.as_u16()),
            Some(&attempt.headers),
        ))
    }

    /// 这条路由支持的对话中途工具更新模式：先看连接配置（选择时按模型信息解析），
    /// 再看目录写死的那条（deepseek-flash = addition-only）。运行时换模型按新模型名重查。
    fn route_tool_update(&self, model: &str) -> Option<ToolUpdate> {
        if model == self.config.model
            && let Some(update) = self.config.tool_update
        {
            return Some(update);
        }
        crate::catalog::declared_tool_update(&self.config.provider, model)
    }

    /// 本次请求该带哪些鉴权头（学到的内存优先，其次连接上记住的，默认两个都发）。
    fn auth_choice(&self, base_url: &str) -> AuthChoice {
        if let Some(header) = self.learned.auth_header(base_url) {
            return AuthChoice::from_learned(&header);
        }
        match self.credentials.auth_header() {
            Some(header) => AuthChoice::from_learned(&header),
            None => AuthChoice::Both,
        }
    }

    /// 发一次 Messages 请求。成功返回响应；失败返回状态/头/错误体（重发判定用）。
    async fn post_messages(
        &self,
        body: Vec<u8>,
        tool_changes: bool,
        api_key: &str,
        choice: AuthChoice,
        cancel: &CancelToken,
    ) -> Result<Attempt, ModelError> {
        let mut request = self
            .http
            .post(self.url.clone())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("anthropic-version", "2023-06-01");
        if tool_changes {
            request = request.header("anthropic-beta", MESSAGES_TOOL_CHANGES_BETA);
        }
        let request = match choice {
            AuthChoice::Both => request
                .header("x-api-key", api_key)
                .header("authorization", format!("Bearer {api_key}")),
            AuthChoice::XApiKey => request.header("x-api-key", api_key),
            AuthChoice::Authorization => {
                request.header("authorization", format!("Bearer {api_key}"))
            }
        };
        let request = request.body(body);
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(aborted("DeepSeek Messages 请求被取消")),
            response = request.send() => response.map_err(transport::transport_error)?,
        };
        if cancel.is_cancelled() {
            return Err(aborted("DeepSeek Messages 请求被取消"));
        }
        let status = response.status();
        if status.is_success() {
            return Ok(Attempt {
                status,
                headers: reqwest::header::HeaderMap::new(),
                raw: serde_json::Value::Null,
                response: Some(response),
            });
        }
        let headers = response.headers().clone();
        // 错误体读取也接取消：极端慢的错误体不能让 Ctrl-C 挂住。
        let text = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                return Err(aborted("DeepSeek Messages 请求被取消"));
            }
            text = response.text() => text.unwrap_or_default(),
        };
        let raw = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Ok(Attempt {
            status,
            headers,
            raw,
            response: None,
        })
    }
}

/// 一次出站尝试。成功时 response 留给 ChunkStream；失败时带状态/头/错误体（重发判定用）。
struct Attempt {
    status: reqwest::StatusCode,
    headers: reqwest::header::HeaderMap,
    raw: serde_json::Value,
    response: Option<reqwest::Response>,
}

/// 取消造成的错误（调用方按「取消」收尾，不当作 turn error）。
fn aborted(message: &str) -> ModelError {
    ModelError::new(ErrorCode::Aborted, message.to_owned())
}

/// 把 base URL 规范化成 Messages 端点：`.../anthropic` -> `.../anthropic/v1/messages`。
fn messages_url(base_url: &str) -> Result<reqwest::Url, ModelError> {
    let trimmed = base_url.trim_end_matches('/');
    let root = if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    };
    reqwest::Url::parse(&format!("{root}/messages")).map_err(|error| {
        ModelError::new(
            ErrorCode::InvalidRequest,
            format!("DeepSeek base URL 非法: {error}"),
        )
    })
}

/// 一次请求的流式响应，按块吐出 provider 无关事件。
pub struct ChunkStream {
    response: reqwest::Response,
    parser: SseParser,
    translator: Translator,
    pending: VecDeque<StreamChunk>,
    /// 翻译时出错但还有已排队的 chunk：先把 chunk 吐完，再报这个错。
    pending_error: Option<ModelError>,
    done: bool,
    cancel: CancelToken,
}

impl ChunkStream {
    fn new(response: reqwest::Response, cancel: CancelToken) -> Self {
        Self {
            response,
            parser: SseParser::default(),
            translator: Translator::new(),
            pending: VecDeque::new(),
            pending_error: None,
            done: false,
            cancel,
        }
    }

    /// 取下一个流事件；流正常结束时返回 `None`。
    pub async fn next(&mut self) -> Option<Result<StreamChunk, ModelError>> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                return Some(Ok(chunk));
            }
            if let Some(error) = self.pending_error.take() {
                self.done = true;
                return Some(Err(error));
            }
            if self.done {
                return None;
            }
            if let Err(error) = self.pump_frames() {
                if let Some(chunk) = self.pending.pop_front() {
                    self.pending_error = Some(error);
                    return Some(Ok(chunk));
                }
                self.done = true;
                return Some(Err(error));
            }
            if let Some(chunk) = self.pending.pop_front() {
                return Some(Ok(chunk));
            }
            if self.done {
                return None;
            }
            let cancel = self.cancel.clone();
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    self.done = true;
                    return Some(Err(aborted("DeepSeek Messages 请求被取消")));
                }
                result = tokio::time::timeout(STREAM_IDLE_TIMEOUT, self.response.chunk()) => {
                    match result {
                        Err(_) => {
                            self.done = true;
                            return Some(Err(ModelError::new(
                                ErrorCode::Timeout,
                                "DeepSeek Messages 流空闲超时（300 秒）",
                            )));
                        }
                        Ok(Err(error)) => {
                            self.done = true;
                            return Some(Err(transport::transport_error(error)));
                        }
                        Ok(Ok(Some(bytes))) => self.parser.push(&bytes),
                        Ok(Ok(None)) => {
                            self.done = true;
                            if !self.translator.is_finished() {
                                return Some(Err(ModelError::new(
                                    ErrorCode::StreamClosed,
                                    "DeepSeek Messages 在 message_stop 之前关闭了流",
                                )));
                            }
                            return None;
                        }
                    }
                }
            }
        }
    }

    fn pump_frames(&mut self) -> Result<(), ModelError> {
        loop {
            let Some(frame) = self.parser.next_frame()? else {
                return Ok(());
            };
            let Some(data) = sse::frame_data(&frame)? else {
                continue;
            };
            let event: serde_json::Value = serde_json::from_str(&data).map_err(|_| {
                ModelError::new(
                    ErrorCode::MalformedResponse,
                    "DeepSeek Messages 的 SSE 事件不是合法 JSON",
                )
            })?;
            let chunks = self.translator.push(event)?;
            self.pending.extend(chunks);
            if self.translator.is_finished() {
                self.done = true;
                return Ok(());
            }
        }
    }
}
