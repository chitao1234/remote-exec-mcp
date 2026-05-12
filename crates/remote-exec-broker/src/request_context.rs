use std::future::Future;

use remote_exec_proto::request_id::RequestId;

#[derive(Debug, Clone)]
pub(crate) struct RequestContext {
    request_id: RequestId,
    tool: &'static str,
}

tokio::task_local! {
    static CURRENT_REQUEST_CONTEXT: RequestContext;
}

impl RequestContext {
    pub(crate) fn new(tool: &'static str) -> Self {
        Self {
            request_id: RequestId::new(),
            tool,
        }
    }

    pub(crate) fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub(crate) fn tool(&self) -> &'static str {
        self.tool
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
