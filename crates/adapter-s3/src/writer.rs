//! Streaming uploads. Small files are one conditional PUT; anything above
//! one part becomes a multipart upload with several parts in flight at once.
//! Every write is conditional (`If-None-Match: *`), so it never overwrites;
//! a failed or abandoned upload is aborted so no orphaned parts are billed.

use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream as SdkStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use domain::{BucketName, ByteSize, VfsError};
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::errors::from_sdk;

/// 8 MiB parts: above S3's 5 MiB minimum, and 10,000 parts reach ~80 GB.
const PART_SIZE: usize = 8 * 1024 * 1024;
/// Parts uploading concurrently per file.
const PARALLEL_PARTS: usize = 4;
const ANY_EXISTING: &str = "*";

struct Upload {
    id: String,
    next_part: i32,
    completed: Vec<CompletedPart>,
    in_flight: FuturesUnordered<JoinHandle<Result<CompletedPart, VfsError>>>,
}

pub(crate) struct S3Sink {
    client: Client,
    bucket: BucketName,
    key: String,
    runtime: Handle,
    buffer: Vec<u8>,
    upload: Option<Upload>,
    finished: bool,
}

impl S3Sink {
    pub(crate) fn new(
        client: Client,
        bucket: BucketName,
        key: String,
        size_hint: Option<ByteSize>,
        runtime: Handle,
    ) -> Self {
        let capacity = size_hint.map_or(0, |s| usize::try_from(s.0).unwrap_or(PART_SIZE).min(PART_SIZE));
        Self { client, bucket, key, runtime, buffer: Vec::with_capacity(capacity), upload: None, finished: false }
    }

    async fn upload_id(&mut self) -> Result<&mut Upload, VfsError> {
        if self.upload.is_none() {
            let output = self
                .client
                .create_multipart_upload()
                .bucket(self.bucket.as_str())
                .key(&self.key)
                .send()
                .await
                .map_err(from_sdk)?;
            let id = output.upload_id().ok_or_else(|| VfsError::Io { message: "no upload id".into() })?.to_owned();
            self.upload = Some(Upload { id, next_part: 1, completed: Vec::new(), in_flight: FuturesUnordered::new() });
        }
        self.upload.as_mut().ok_or(VfsError::Conflict)
    }

    async fn send_part(&mut self, part: Vec<u8>) -> Result<(), VfsError> {
        let (client, bucket, key, runtime) =
            (self.client.clone(), self.bucket.clone(), self.key.clone(), self.runtime.clone());
        let upload = self.upload_id().await?;
        let (id, number) = (upload.id.clone(), upload.next_part);
        upload.next_part += 1;
        upload.in_flight.push(runtime.spawn(async move {
            let output = client
                .upload_part()
                .bucket(bucket.as_str())
                .key(key)
                .upload_id(id)
                .part_number(number)
                .body(SdkStream::from(part))
                .send()
                .await
                .map_err(from_sdk)?;
            Ok(CompletedPart::builder().set_e_tag(output.e_tag().map(str::to_owned)).part_number(number).build())
        }));
        while upload.in_flight.len() >= PARALLEL_PARTS {
            collect_one(upload).await?;
        }
        Ok(())
    }

    async fn finish_multipart(&mut self) -> Result<(), VfsError> {
        let last = std::mem::take(&mut self.buffer);
        if !last.is_empty() {
            self.send_part(last).await?;
        }
        let upload = self.upload.as_mut().ok_or(VfsError::Conflict)?;
        while !upload.in_flight.is_empty() {
            collect_one(upload).await?;
        }
        let mut parts = std::mem::take(&mut upload.completed);
        parts.sort_by_key(|part| part.part_number());
        self.client
            .complete_multipart_upload()
            .bucket(self.bucket.as_str())
            .key(&self.key)
            .upload_id(&upload.id)
            .if_none_match(ANY_EXISTING)
            .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
            .send()
            .await
            .map_err(from_sdk)
            .map(drop)
    }

    async fn put_whole(&mut self) -> Result<(), VfsError> {
        let body = std::mem::take(&mut self.buffer);
        self.client
            .put_object()
            .bucket(self.bucket.as_str())
            .key(&self.key)
            .if_none_match(ANY_EXISTING)
            .body(SdkStream::from(body))
            .send()
            .await
            .map_err(from_sdk)
            .map(drop)
    }

    fn abort_request(&self) -> Option<BoxFuture<'static, ()>> {
        let upload = self.upload.as_ref()?;
        let request =
            self.client.abort_multipart_upload().bucket(self.bucket.as_str()).key(&self.key).upload_id(&upload.id);
        Some(async move { drop(request.send().await) }.boxed())
    }
}

async fn collect_one(upload: &mut Upload) -> Result<(), VfsError> {
    if let Some(joined) = upload.in_flight.next().await {
        let part = joined.map_err(|join| VfsError::Io { message: join.to_string() })??;
        upload.completed.push(part);
    }
    Ok(())
}

impl application::ports::WriteSink for S3Sink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            self.buffer.extend(chunk);
            while self.buffer.len() >= PART_SIZE {
                let part: Vec<u8> = self.buffer.drain(..PART_SIZE).collect();
                self.send_part(part).await?;
            }
            Ok(())
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let result = if self.upload.is_some() { self.finish_multipart().await } else { self.put_whole().await };
            if result.is_ok() {
                self.finished = true;
            }
            // On failure, dropping `self` aborts the multipart upload.
            result
        }
        .boxed()
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        self.finished = true;
        self.abort_request().unwrap_or_else(|| futures::future::ready(()).boxed())
    }
}

impl Drop for S3Sink {
    fn drop(&mut self) {
        if !self.finished
            && let Some(abort) = self.abort_request()
        {
            self.runtime.spawn(abort);
        }
    }
}
