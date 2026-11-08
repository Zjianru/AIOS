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
            // 根宿主自己填统计/汇率/币种（需要 data_dir + Keyring）。
            Call::Status(request) => self.host_status(request).await.map(Response::Status),
            // 余额/余量需要 Keyring 签发 key。
            Call::ConnectionQuota(request) => self
                .connection_quota(request)
                .await
                .map(Response::ConnectionQuota),
            // 写非密钥设置。
            Call::SettingsUpdate(request) => self
                .settings_update(request)
                .await
                .map(Response::SettingsUpdate),
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
