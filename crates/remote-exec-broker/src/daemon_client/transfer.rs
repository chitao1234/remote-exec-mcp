use futures_util::TryStreamExt;

use remote_exec_proto::rpc::{
    TransferExportMetadata, TransferExportRequest, TransferImportRequest, TransferImportResponse,
    TransferPathInfoRequest, TransferPathInfoResponse,
};
use reqwest::header::CONTENT_LENGTH;

use crate::tools::transfer::codec;

use super::{
    DaemonClient, DaemonClientError, RpcCallContext, RpcCallKind, RpcErrorDecodePolicy,
    TransferExportResponse, TransferExportStream,
};

impl DaemonClient {
    pub async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        self.post("/v1/transfer/path-info", req).await
    }

    pub async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferExportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            "starting daemon transfer export"
        );
        let TransferExportStream {
            source_type,
            response,
        } = self.transfer_export_stream(req).await?;
        self.write_transfer_export_archive(archive_path, response)
            .await?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer export completed"
        );
        Ok(TransferExportResponse { source_type })
    }

    pub async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        let started = std::time::Instant::now();
        let response = self.send_transfer_export_request(req, started).await?;
        let metadata = self.transfer_export_metadata(req, response.headers())?;
        Ok(TransferExportStream {
            source_type: metadata.source_type,
            response,
        })
    }

    pub async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        let (file_len, body) = open_transfer_import_body(archive_path).await?;
        let response = self
            .send_transfer_import_request(req, Some(file_len), body, started)
            .await?;
        let summary = self
            .decode_transfer_import_response(req, started, response)
            .await?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            destination_path = %req.destination_path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer import completed"
        );
        Ok(summary)
    }

    pub async fn transfer_import_from_body(
        &self,
        req: &TransferImportRequest,
        body: reqwest::Body,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        let response = self
            .send_transfer_import_request(req, None, body, started)
            .await?;
        self.decode_transfer_import_response(req, started, response)
            .await
    }

    async fn send_transfer_export_request(
        &self,
        req: &TransferExportRequest,
        started: std::time::Instant,
    ) -> Result<reqwest::Response, DaemonClientError> {
        let context = RpcCallContext::path(
            &self.target_name,
            &self.base_url,
            started,
            RpcCallKind::TransferExport,
            req.path.as_str(),
        );
        self.send_request_with_policy(
            self.request("/v1/transfer/export").json(req).send(),
            RpcErrorDecodePolicy::Lenient,
            |err| context.log_transport_error(err),
            |status| context.log_status_error(status),
        )
        .await
    }

    fn transfer_export_metadata(
        &self,
        req: &TransferExportRequest,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<TransferExportMetadata, DaemonClientError> {
        let metadata = codec::parse_export_metadata(headers)?;
        if metadata.compression != req.compression {
            return Err(DaemonClientError::Decode(anyhow::anyhow!(
                "target `{}` returned transfer compression `{}` for requested `{}`",
                self.target_name,
                codec::compression_header_value(&metadata.compression),
                codec::compression_header_value(&req.compression)
            )));
        }

        Ok(metadata)
    }

    async fn write_transfer_export_archive(
        &self,
        archive_path: &std::path::Path,
        response: reqwest::Response,
    ) -> Result<(), DaemonClientError> {
        let mut file = tokio::fs::File::create(archive_path)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        let mut stream = tokio_util::io::StreamReader::new(
            response.bytes_stream().map_err(std::io::Error::other),
        );
        tokio::io::copy(&mut stream, &mut file)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Ok(())
    }

    async fn send_transfer_import_request(
        &self,
        req: &TransferImportRequest,
        file_len: Option<u64>,
        body: reqwest::Body,
        started: std::time::Instant,
    ) -> Result<reqwest::Response, DaemonClientError> {
        let context = RpcCallContext::destination_path(
            &self.target_name,
            &self.base_url,
            started,
            RpcCallKind::TransferImport,
            req.destination_path.as_str(),
        );
        let mut request =
            codec::apply_import_headers(self.request("/v1/transfer/import"), &req.metadata());
        if let Some(file_len) = file_len {
            request = request.header(CONTENT_LENGTH, file_len);
        }
        self.send_request_with_policy(
            request.body(body).send(),
            RpcErrorDecodePolicy::Lenient,
            |err| context.log_transport_error(err),
            |status| context.log_status_error(status),
        )
        .await
    }

    async fn decode_transfer_import_response(
        &self,
        req: &TransferImportRequest,
        started: std::time::Instant,
        response: reqwest::Response,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let context = RpcCallContext::destination_path(
            &self.target_name,
            &self.base_url,
            started,
            RpcCallKind::TransferImport,
            req.destination_path.as_str(),
        );
        self.decode_json_response(
            response,
            |err| context.log_read_error(err),
            |err| context.log_decode_error(err),
        )
        .await
    }
}

async fn open_transfer_import_body(
    archive_path: &std::path::Path,
) -> Result<(u64, reqwest::Body), DaemonClientError> {
    let file = tokio::fs::File::open(archive_path)
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    let file_len = file
        .metadata()
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?
        .len();
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    Ok((file_len, body))
}
