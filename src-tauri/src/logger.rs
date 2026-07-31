use crate::db::{Account, Db, RequestLogStart, RequestLogUsage};
use rusqlite::Result as SqlResult;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::warn;

#[derive(Debug, Clone, Copy, Default)]
struct RequestLogState {
    completed: bool,
    http_status: Option<i64>,
    ttfb_ms: Option<i64>,
    usage: RequestLogUsage,
}

#[derive(Debug)]
struct RequestLogInner {
    db: Arc<Db>,
    id: i64,
    started_at: Instant,
    state: Mutex<RequestLogState>,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestLogHandle {
    inner: Arc<RequestLogInner>,
}

impl RequestLogHandle {
    pub(crate) fn begin(db: Arc<Db>, start: &RequestLogStart) -> SqlResult<Self> {
        let id = db.insert_request_log(start)?;
        Ok(Self {
            inner: Arc::new(RequestLogInner {
                db,
                id,
                started_at: Instant::now(),
                state: Mutex::new(RequestLogState::default()),
            }),
        })
    }

    pub(crate) fn mark_response(&self, http_status: u16) {
        let ttfb_ms = elapsed_millis(self.inner.started_at);
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.completed {
                return;
            }
            state.http_status = Some(i64::from(http_status));
            state.ttfb_ms.get_or_insert(ttfb_ms);
        }
        if let Err(error) =
            self.inner
                .db
                .mark_request_log_response(self.inner.id, http_status, ttfb_ms)
        {
            warn!(log_id = self.inner.id, %error, "更新请求日志响应时间失败");
        }
    }

    pub(crate) fn record_usage(&self, usage: RequestLogUsage) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.completed {
            state.usage = usage;
        }
    }

    pub(crate) fn finish(&self, status: &str, message: Option<&str>) {
        let snapshot = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.completed {
                return;
            }
            state.completed = true;
            *state
        };
        if let Err(error) = self.inner.db.complete_request_log(
            self.inner.id,
            status,
            snapshot.http_status,
            snapshot.ttfb_ms,
            elapsed_millis(self.inner.started_at),
            snapshot.usage,
            message.unwrap_or_default(),
        ) {
            warn!(log_id = self.inner.id, %error, "完成请求日志失败");
        }
    }
}

pub(crate) fn begin_probe(db: Arc<Db>, account: &Account, path: &str) -> Option<RequestLogHandle> {
    let start = RequestLogStart {
        request_id: uuid::Uuid::new_v4().simple().to_string(),
        attempt_index: 1,
        account_id: Some(account.id.clone()),
        account_name: account.name.clone(),
        account_type: account.account_type.clone(),
        source: "probe".to_string(),
        method: "GET".to_string(),
        path: path.to_string(),
        endpoint_family: "models".to_string(),
        model: String::new(),
        streaming: false,
    };
    match RequestLogHandle::begin(db, &start) {
        Ok(handle) => Some(handle),
        Err(error) => {
            warn!(account_id = %account.id, %error, "创建渠道探测日志失败");
            None
        }
    }
}

impl Drop for RequestLogInner {
    fn drop(&mut self) {
        let snapshot = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.completed {
                return;
            }
            state.completed = true;
            *state
        };
        if let Err(error) = self.db.complete_request_log(
            self.id,
            "cancelled",
            snapshot.http_status,
            snapshot.ttfb_ms,
            elapsed_millis(self.started_at),
            snapshot.usage,
            "请求在响应完成前中断",
        ) {
            warn!(log_id = self.id, %error, "取消请求日志失败");
        }
    }
}

fn elapsed_millis(started_at: Instant) -> i64 {
    started_at.elapsed().as_millis().min(i64::MAX as u128) as i64
}
