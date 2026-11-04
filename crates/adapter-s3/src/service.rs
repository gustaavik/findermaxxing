//! Connected accounts and the `s3://` provider factory. One object plays both
//! roles because opening a bucket mount needs the account's client.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use application::ports::{BucketInfo, MountHandle, ProviderFactory, S3Connector, Secret};
use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Credentials;
use domain::{
    AccountId, Addressing, BucketName, Capabilities, Capability, MountId, PathError, S3_SCHEME, S3Account, S3Auth,
    S3Location, Scheme, VfsError,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::bucket::BucketFs;
use crate::errors::from_sdk;

/// Custom endpoints (MinIO, R2…) still need *some* signing region.
const DEFAULT_SIGNING_REGION: &str = "us-east-1";
const CREDENTIALS_PROVIDER_NAME: &str = "FinderMaxxing";

const S3_SEMANTICS: Capabilities =
    Capabilities::empty().with(Capability::Metered).with(Capability::ServerSideCopy).with(Capability::ResumableUpload);

struct Connected {
    client: Client,
    region: String,
    /// Learned from ListBuckets; a bucket in another region needs a client for it.
    bucket_regions: HashMap<String, String>,
}

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    cache_root: PathBuf,
    accounts: RwLock<HashMap<AccountId, Connected>>,
}

/// Cheap to clone; clones share connections.
#[derive(Clone)]
pub struct S3Service {
    inner: Arc<Inner>,
}

impl S3Service {
    /// `cache_root` holds downloaded copies for previews and "open".
    ///
    /// # Errors
    /// Never in practice; the scheme literal is validated like any other.
    pub fn new(runtime: Handle, cache_root: PathBuf) -> Result<Self, PathError> {
        let scheme = Scheme::new(S3_SCHEME)?;
        Ok(Self { inner: Arc::new(Inner { scheme, runtime, cache_root, accounts: RwLock::default() }) })
    }

    /// The bucket's roles, for callers that need the concrete type (tests).
    ///
    /// # Errors
    /// `Unsupported(List)` if the account isn't connected.
    pub fn open_bucket(&self, location: &S3Location) -> Result<BucketFs, VfsError> {
        let (client, _) = self.client_for(location).ok_or(VfsError::Unsupported(Capability::List))?;
        let cache = self.inner.cache_root.join(location.account.to_string()).join(location.bucket.as_str());
        Ok(BucketFs::new(client, location.bucket.clone(), self.inner.runtime.clone(), cache))
    }

    fn client_for(&self, location: &S3Location) -> Option<(Client, String)> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let connected = accounts.get(&location.account)?;
        match connected.bucket_regions.get(location.bucket.as_str()) {
            Some(region) if *region != connected.region => {
                let config = connected.client.config().to_builder().region(Region::new(region.clone())).build();
                Some((Client::from_conf(config), region.clone()))
            }
            _ => Some((connected.client.clone(), connected.region.clone())),
        }
    }
}

impl std::fmt::Debug for S3Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Service").finish_non_exhaustive()
    }
}

async fn build_client(account: &S3Account, secret: Option<&Secret>) -> Result<(Client, String), VfsError> {
    let region =
        account.region.clone().or_else(|| account.endpoint.as_ref().map(|_| DEFAULT_SIGNING_REGION.to_owned()));
    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(region) = &region {
        loader = loader.region(Region::new(region.clone()));
    }
    loader = match (&account.auth, secret) {
        (S3Auth::DefaultChain, _) => loader,
        (S3Auth::Profile(profile), _) => loader.profile_name(profile),
        (S3Auth::AccessKey { access_key_id }, Some(secret)) => loader.credentials_provider(Credentials::new(
            access_key_id,
            secret.expose(),
            None,
            None,
            CREDENTIALS_PROVIDER_NAME,
        )),
        (S3Auth::AccessKey { .. }, None) => return Err(VfsError::PermissionDenied),
    };
    if let Some(endpoint) = &account.endpoint {
        loader = loader.endpoint_url(endpoint);
    }
    let shared = loader.load().await;
    let resolved_region = shared.region().map_or_else(|| DEFAULT_SIGNING_REGION.to_owned(), ToString::to_string);
    let config = aws_sdk_s3::config::Builder::from(&shared)
        .force_path_style(account.addressing == Addressing::PathStyle)
        .build();
    Ok((Client::from_conf(config), resolved_region))
}

impl S3Connector for S3Service {
    fn connect(&self, account: &S3Account, secret: Option<Secret>) -> BoxFuture<'static, Result<(), VfsError>> {
        let (service, account) = (self.clone(), account.clone());
        async move {
            let (client, region) = build_client(&account, secret.as_ref()).await?;
            let connected = Connected { client, region, bucket_regions: HashMap::new() };
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(account.id, connected);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }

    fn list_buckets(&self, id: AccountId) -> BoxFuture<'static, Result<Vec<BucketInfo>, VfsError>> {
        let service = self.clone();
        async move {
            let client = {
                let accounts = service.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
                accounts.get(&id).map(|c| c.client.clone()).ok_or(VfsError::NotFound)?
            };
            let output = client.list_buckets().send().await.map_err(from_sdk)?;
            let buckets: Vec<BucketInfo> = output
                .buckets()
                .iter()
                .filter_map(|bucket| {
                    let name = BucketName::new(bucket.name()?).ok()?;
                    Some(BucketInfo { name, region: bucket.bucket_region().map(str::to_owned) })
                })
                .collect();
            if let Some(connected) = service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).get_mut(&id)
            {
                for bucket in &buckets {
                    if let Some(region) = &bucket.region {
                        connected.bucket_regions.insert(bucket.name.to_string(), region.clone());
                    }
                }
            }
            Ok(buckets)
        }
        .boxed()
    }
}

impl ProviderFactory for S3Service {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = S3Location::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let region = self.client_for(&location).map(|(_, region)| region);
        let bucket = Arc::new(self.open_bucket(&location)?);
        Ok(MountHandle::builder(S3_SEMANTICS)
            .region(region)
            .lister(bucket.clone())
            .copier(bucket.clone())
            .mover(bucket.clone())
            .remover(bucket.clone())
            .folder_maker(bucket.clone())
            .reader(bucket.clone())
            .writer(bucket.clone())
            .materializer(bucket.clone())
            .metadata_editor(bucket.clone())
            .presigner(bucket.clone())
            .version_lister(bucket)
            .build())
    }
}
