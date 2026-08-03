use futures_util::{StreamExt, TryStream, TryStreamExt};

use crate::tools::transfer::codec;
use remote_exec_proto::rpc::{
    TRANSFER_STREAM_PROTOCOL_VERSION, TRANSFER_STREAM_VERSION_HEADER, TransferExportMetadata,
    TransferExportRequest, TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};
use remote_exec_proto::transfer::TransferSourceType;

use super::{
    DaemonClient, DaemonClientError, RpcCallContext, RpcCallKind, RpcErrorDecodePolicy,
    transfer_stream,
};

#[derive(Debug)]
pub struct TransferExportStream {
    pub source_type: TransferSourceType,
    pub(super) response: reqwest::Response,
}

impl TransferExportStream {
    pub fn into_async_read(self) -> impl tokio::io::AsyncRead + Send + Unpin + 'static {
        let stream =
            transfer_stream::decode_response_body(self.response).map_err(std::io::Error::other);
        tokio_util::io::StreamReader::new(stream.boxed())
    }
}

impl DaemonClient {
    pub async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        self.post("/v1/transfer/path-info", req).await
    }

    pub async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        let started = std::time::Instant::now();
        let (response, connection_generation) =
            self.send_transfer_export_request(req, started).await?;
        self.record_connection_success(connection_generation);
        let metadata = self.transfer_export_metadata(req, response.headers())?;
        Ok(TransferExportStream {
            source_type: metadata.source_type,
            response,
        })
    }

    pub async fn transfer_import_from_archive_stream<S, E>(
        &self,
        req: &TransferImportRequest,
        stream: S,
    ) -> Result<TransferImportResponse, DaemonClientError>
    where
        S: TryStream<Ok = bytes::Bytes, Error = E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let started = std::time::Instant::now();
        let body = transfer_stream::encode_request_body(stream);
        let (response, connection_generation) = self
            .send_transfer_import_request(req, body, started)
            .await?;
        self.decode_transfer_import_response(req, started, response, connection_generation)
            .await
    }

    async fn send_transfer_export_request(
        &self,
        req: &TransferExportRequest,
        started: std::time::Instant,
    ) -> Result<(reqwest::Response, u64), DaemonClientError> {
        let context = RpcCallContext::path(
            &self.target_name,
            &self.base_url,
            started,
            RpcCallKind::TransferExport,
            req.path.as_str(),
        );
        let (request, connection_generation) = self.request_with_generation("/v1/transfer/export");
        let response = self
            .send_request_with_policy(
                request
                    .header(
                        TRANSFER_STREAM_VERSION_HEADER,
                        TRANSFER_STREAM_PROTOCOL_VERSION.to_string(),
                    )
                    .json(req)
                    .send(),
                connection_generation,
                "daemon transfer export request",
                RpcErrorDecodePolicy::Lenient,
                |err| context.log_transport_error(err),
                |status| context.log_status_error(status),
            )
            .await?;
        Ok((response, connection_generation))
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

    async fn send_transfer_import_request(
        &self,
        req: &TransferImportRequest,
        body: reqwest::Body,
        started: std::time::Instant,
    ) -> Result<(reqwest::Response, u64), DaemonClientError> {
        let context = RpcCallContext::destination_path(
            &self.target_name,
            &self.base_url,
            started,
            RpcCallKind::TransferImport,
            req.destination_path.as_str(),
        );
        let (request, connection_generation) = self.request_with_generation("/v1/transfer/import");
        let request = codec::apply_import_headers(request, req);
        let response = self
            .send_request_with_policy(
                request.body(body).send(),
                connection_generation,
                "daemon transfer import request",
                RpcErrorDecodePolicy::Lenient,
                |err| context.log_transport_error(err),
                |status| context.log_status_error(status),
            )
            .await?;
        Ok((response, connection_generation))
    }

    async fn decode_transfer_import_response(
        &self,
        req: &TransferImportRequest,
        started: std::time::Instant,
        response: reqwest::Response,
        connection_generation: u64,
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
            connection_generation,
            "daemon transfer import response",
            |err| context.log_read_error(err),
            |err| context.log_decode_error(err),
        )
        .await
    }
}
