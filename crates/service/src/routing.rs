//! 根宿主的调用入口：把 Call 分派到根会话或裸宿主，并转发 follow/shutdown。

use crate::error::ServiceError;
use crate::protocol::{
    Call, FollowRequest, FollowResponse, MonitorReportResponse, RecheckResponse, Response,
};
use crate::root_lifecycle::RootHost;

impl RootHost {
    /// 唯一调用入口；根会话不可用时除 recheck 外一律拒绝。
    pub async fn dispatch(&self, call: Call) -> Result<Response, ServiceError> {
        if matches!(call, Call::Recheck(_)) {
            let root = self.recheck().await;
            let host = self.info().host;
            return Ok(Response::Recheck(RecheckResponse { root, host }));
        }
        match call {
            // 发消息前把附件 id 解析成只读副本路径（文字/粘贴/文件拼成块）。
            Call::Send(request) => {
                let message = self.user_message_from(&request)?;
                let guard = self.inner.lock().await;
                match guard.as_ref() {
                    Some(host) => host
                        .send_message(&request.request_id, message, request.delivery)
                        .await
                        .map(Response::Send),
                    None => Err(ServiceError::RootUnavailable(
                        self.state.lock().expect("root state 锁被毒化").clone(),
                    )),
                }
            }
            Call::ModelSettings(_) => self.model_settings().await.map(Response::ModelSettings),
            Call::DisabledModelsSet(request) => self
                .disabled_models_set(request)
                .await
                .map(Response::DisabledModelsSet),
            Call::DisabledModelsCheck(request) => self
                .disabled_models_check(request)
                .await
                .map(Response::DisabledModelsCheck),
            Call::ConnectionSave(request) => self
                .connection_save(request)
                .await
                .map(Response::ConnectionSave),
            Call::ConnectionDelete(request) => self
                .connection_delete(request)
                .await
                .map(Response::ConnectionDelete),
            Call::ConnectionVerify(request) => self
                .connection_verify(request)
                .await
                .map(Response::ConnectionVerify),
            Call::AvailableModels(request) => self
                .available_models(request)
                .await
                .map(Response::AvailableModels),
            Call::ConnectionUpdate(request) => self
                .connection_update(request)
                .await
                .map(Response::ConnectionUpdate),
            Call::ConnectionRevoke(request) => self
                .connection_revoke(request)
                .await
                .map(Response::ConnectionRevoke),
            Call::KeyringRelocate(request) => self
                .keyring_relocate(request)
                .await
                .map(Response::KeyringRelocate),
            Call::KeyringReopen(request) => self
                .keyring_reopen(request)
                .await
                .map(Response::KeyringReopen),
            Call::KeyringRestart(_) => self.keyring_restart().await.map(Response::KeyringRestart),
            Call::Issues(_) => self.issues().await.map(Response::Issues),
            Call::FirstUse(_) => self.first_use().await.map(Response::FirstUse),
            Call::SessionModelSelect(request) => self
                .session_model_select(request)
                .await
                .map(Response::SessionModelSelect),
            // 确认卡：自己取日志快照，不在持锁时跑动作（换模型可能重开根会话）。
            Call::CardAct(request) => self.card_act(request).await.map(Response::CardAct),
            // 根宿主自己填统计/汇率/币种（需要 data_dir + Keyring）。
            Call::Status(request) => self.host_status(request).await.map(Response::Status),
            // 余额/余量需要 Keyring 签发 key。
            Call::ConnectionQuota(request) => self
                .connection_quota(request)
                .await
                .map(Response::ConnectionQuota),
            // 设置页「联网搜索」：Exa 凭证的四个操作。
            Call::ExaKey(_) => self.exa_key().await.map(Response::ExaKey),
            Call::ExaKeySave(request) => self.exa_key_save(request).await.map(Response::ExaKeySave),
            Call::ExaKeyCheck(_) => self.exa_key_check().await.map(Response::ExaKeyCheck),
            Call::ExaKeyDelete(_) => self.exa_key_delete().await.map(Response::ExaKeyDelete),
            // 写非密钥设置。
            Call::SettingsUpdate(request) => self
                .settings_update(request)
                .await
                .map(Response::SettingsUpdate),
            // task-read：任务对话的只读快照（任务库在任务执行者手里）。
            Call::TaskRead(request) => match self.task_runtime() {
                Some(runtime) => runtime.task_read(request).map(Response::TaskRead),
                None => Err(ServiceError::TaskUnavailable(
                    "任务库没开，任务对话看不了".to_owned(),
                )),
            },
            // 监控上报不依赖根会话（页面报错时根可能正好不可用）；记录在 web 载体。
            Call::MonitorReport(_) => {
                Ok(Response::MonitorReport(MonitorReportResponse { ok: true }))
            }
            // 附件仓库（文字文件只读副本）。
            Call::AttachmentPut(request) => self
                .attachment_put(request)
                .await
                .map(Response::AttachmentPut),
            Call::AttachmentGet(request) => self
                .attachment_get(request)
                .await
                .map(Response::AttachmentGet),
            // @ 文件候选（按会话工作目录）。
            Call::FileReferencesList(request) => self
                .file_references_list(request)
                .await
                .map(Response::FileReferencesList),
            other => {
                let guard = self.inner.lock().await;
                match guard.as_ref() {
                    Some(host) => host.dispatch(other).await,
                    None => Err(ServiceError::RootUnavailable(
                        self.state.lock().expect("root state 锁被毒化").clone(),
                    )),
                }
            }
        }
    }

    /// `/ws?task=`：任务当前 Attempt 的补齐与 live 订阅（任务库在任务执行者手里）。
    pub async fn task_stream(
        &self,
        task_id: &str,
        last_seq: u64,
    ) -> Result<crate::TaskStream, ServiceError> {
        match self.task_runtime() {
            Some(runtime) => runtime.task_stream(task_id, last_seq).await,
            None => Err(ServiceError::TaskUnavailable(
                "任务库没开，任务对话看不了".to_owned(),
            )),
        }
    }

    pub async fn follow(&self, request: FollowRequest) -> Result<FollowResponse, ServiceError> {
        let guard = self.inner.lock().await;
        match guard.as_ref() {
            Some(host) => host.follow(request).await,
            None => Err(ServiceError::RootUnavailable(
                self.state.lock().expect("root state 锁被毒化").clone(),
            )),
        }
    }

    pub async fn shutdown(&self) {
        let guard = self.inner.lock().await;
        if let Some(host) = guard.as_ref() {
            host.shutdown().await;
        }
    }
}
