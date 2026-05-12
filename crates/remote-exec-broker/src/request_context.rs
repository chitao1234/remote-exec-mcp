use std::future::Future;
use std::sync::{Arc, Mutex};

use remote_exec_proto::request_id::RequestId;

#[derive(Debug, Clone)]
pub(crate) struct RequestContext {
    request_id: RequestId,
    tool: &'static str,
    target: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestContextSnapshot {
    request_id: RequestId,
    tool: &'static str,
    target: Option<String>,
}

tokio::task_local! {
    static CURRENT_REQUEST_CONTEXT: RequestContext;
}

impl RequestContext {
    pub(crate) fn new(tool: &'static str) -> Self {
        Self {
            request_id: RequestId::new(),
            tool,
            target: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub(crate) fn tool(&self) -> &'static str {
        self.tool
    }

    pub(crate) fn set_target(&self, target: impl Into<String>) {
        let target = target.into();
        if target.is_empty() {
            return;
        }
        *lock_unpoisoned(&self.target) = Some(target);
    }

    fn snapshot(&self) -> RequestContextSnapshot {
        RequestContextSnapshot {
            request_id: self.request_id.clone(),
            tool: self.tool,
            target: lock_unpoisoned(&self.target).clone(),
        }
    }
}

impl RequestContextSnapshot {
    pub(crate) fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub(crate) fn tool(&self) -> &'static str {
        self.tool
    }

    pub(crate) fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
}

pub(crate) async fn scope<F>(context: RequestContext, future: F) -> F::Output
where
    F: Future,
{
    CURRENT_REQUEST_CONTEXT.scope(context, future).await
}

pub(crate) fn current_request_id() -> Option<RequestId> {
    CURRENT_REQUEST_CONTEXT
        .try_with(|context| context.request_id.clone())
        .ok()
}

pub(crate) fn current() -> Option<RequestContextSnapshot> {
    CURRENT_REQUEST_CONTEXT
        .try_with(RequestContext::snapshot)
        .ok()
}

pub(crate) fn set_current_target(target: impl Into<String>) {
    let target = target.into();
    let _ = CURRENT_REQUEST_CONTEXT.try_with(|context| context.set_target(target));
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
