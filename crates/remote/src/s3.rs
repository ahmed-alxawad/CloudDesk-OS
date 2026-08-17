use std::path::{Component, Path, PathBuf};

use aws_config::SdkConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use clouddesk_vfs::{EntryKind, ProviderFeature, VfsEntry, VfsError, VfsProvider};
use tokio::runtime::Handle;

pub struct S3Provider {
    client: Client,
    bucket: String,
    handle: Handle,
}

impl S3Provider {
    #[must_use]
    pub fn new(config: &SdkConfig, bucket: String, handle: Handle) -> Self {
        let mut config_builder = aws_sdk_s3::config::Builder::from(config);
        config_builder = config_builder.force_path_style(true);

        Self {
            client: Client::from_conf(config_builder.build()),
            bucket,
            handle,
        }
    }

    fn normalize_virtual_path(path: &str) -> Result<String, VfsError> {
        if path.as_bytes().contains(&0) {
            return Err(VfsError::InvalidPath);
        }
        let mut normalized = PathBuf::new();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(value) => normalized.push(value),
                Component::CurDir | Component::RootDir => {}
                Component::ParentDir | Component::Prefix(_) => return Err(VfsError::Traversal),
            }
        }
        Ok(normalized.to_str().unwrap_or("").to_string())
    }
}

impl VfsProvider for S3Provider {
    fn capabilities(&self) -> Vec<ProviderFeature> {
        vec![
            ProviderFeature::Read,
            ProviderFeature::Write,
            ProviderFeature::Trash,
        ]
    }

    fn list(&self, path: &str) -> Result<Vec<VfsEntry>, VfsError> {
        let prefix = Self::normalize_virtual_path(path)?;
        let mut prefix = prefix.trim_end_matches('/').to_string();
        if !prefix.is_empty() {
            prefix.push('/');
        }

        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let out = self
                    .client
                    .list_objects_v2()
                    .bucket(&bucket)
                    .prefix(&prefix)
                    .delimiter("/")
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                let mut entries = Vec::new();
                for prefix_obj in out.common_prefixes() {
                    let dir_path = prefix_obj.prefix().unwrap_or("");
                    let name = dir_path
                        .trim_end_matches('/')
                        .split('/')
                        .next_back()
                        .unwrap_or("")
                        .to_string();
                    entries.push(VfsEntry {
                        name,
                        path: format!("/{}", dir_path.trim_end_matches('/')),
                        kind: EntryKind::Directory,
                        size: 0,
                        modified_at: None,
                        mode: 0,
                        uid: 0,
                        gid: 0,
                    });
                }

                for obj in out.contents() {
                    let obj_path = obj.key().unwrap_or("");
                    if obj_path == prefix {
                        continue;
                    } // Exclude the directory itself if it exists as an object
                    let name = obj_path.split('/').next_back().unwrap_or("").to_string();
                    let size = u64::try_from(obj.size().unwrap_or(0).max(0)).unwrap_or(0);
                    let modified_at = obj
                        .last_modified()
                        .map(aws_sdk_s3::primitives::DateTime::secs);
                    entries.push(VfsEntry {
                        name,
                        path: format!("/{obj_path}"),
                        kind: EntryKind::File,
                        size,
                        modified_at,
                        mode: 0,
                        uid: 0,
                        gid: 0,
                    });
                }
                Ok(entries)
            })
        })
    }

    fn stat(&self, path: &str) -> Result<VfsEntry, VfsError> {
        let key = Self::normalize_virtual_path(path)?;
        if key.is_empty() {
            return Ok(VfsEntry {
                name: String::new(),
                path: "/".to_string(),
                kind: EntryKind::Directory,
                size: 0,
                modified_at: None,
                mode: 0,
                uid: 0,
                gid: 0,
            });
        }

        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let out = self
                    .client
                    .head_object()
                    .bucket(&bucket)
                    .key(&key)
                    .send()
                    .await;

                if let Ok(head) = out {
                    let name = key.split('/').next_back().unwrap_or("").to_string();
                    Ok(VfsEntry {
                        name,
                        path: format!("/{key}"),
                        kind: EntryKind::File, // Assuming it's a file
                        size: u64::try_from(head.content_length().unwrap_or(0).max(0)).unwrap_or(0),
                        modified_at: head
                            .last_modified()
                            .map(aws_sdk_s3::primitives::DateTime::secs),
                        mode: 0,
                        uid: 0,
                        gid: 0,
                    })
                } else {
                    // Try to see if it's a directory
                    let dir_key = format!("{key}/");
                    let list = self
                        .client
                        .list_objects_v2()
                        .bucket(&bucket)
                        .prefix(&dir_key)
                        .max_keys(1)
                        .send()
                        .await
                        .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                    if list.contents().is_empty() && list.common_prefixes().is_empty() {
                        Err(VfsError::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "Not Found",
                        )))
                    } else {
                        let name = key.split('/').next_back().unwrap_or("").to_string();
                        Ok(VfsEntry {
                            name,
                            path: format!("/{key}"),
                            kind: EntryKind::Directory,
                            size: 0,
                            modified_at: None,
                            mode: 0,
                            uid: 0,
                            gid: 0,
                        })
                    }
                }
            })
        })
    }

    fn create_directory(&self, path: &str) -> Result<(), VfsError> {
        let key = Self::normalize_virtual_path(path)?;
        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let dir_key = format!("{key}/");
                self.client
                    .put_object()
                    .bucket(&bucket)
                    .key(&dir_key)
                    .body(ByteStream::from_static(b""))
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                Ok(())
            })
        })
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from_key = Self::normalize_virtual_path(from)?;
        let to_key = Self::normalize_virtual_path(to)?;
        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                // S3 doesn't have rename, we must CopyObject then DeleteObject
                let source = format!("{bucket}/{from_key}");
                self.client
                    .copy_object()
                    .bucket(&bucket)
                    .key(&to_key)
                    .copy_source(&source)
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                self.client
                    .delete_object()
                    .bucket(&bucket)
                    .key(&from_key)
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                Ok(())
            })
        })
    }

    fn copy_file(&self, from: &str, to: &str) -> Result<u64, VfsError> {
        let from_key = Self::normalize_virtual_path(from)?;
        let to_key = Self::normalize_virtual_path(to)?;
        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let source = format!("{bucket}/{from_key}");
                self.client
                    .copy_object()
                    .bucket(&bucket)
                    .key(&to_key)
                    .copy_source(&source)
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                Ok(0)
            })
        })
    }

    fn trash(&self, path: &str) -> Result<String, VfsError> {
        let key = Self::normalize_virtual_path(path)?;
        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.client
                    .delete_object()
                    .bucket(&bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                Ok("deleted".to_string())
            })
        })
    }

    fn read_limited(&self, path: &str, maximum_bytes: usize) -> Result<Vec<u8>, VfsError> {
        let key = Self::normalize_virtual_path(path)?;
        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let range = format!("bytes=0-{}", maximum_bytes - 1);
                let out = self
                    .client
                    .get_object()
                    .bucket(&bucket)
                    .key(&key)
                    .range(range)
                    .send()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                let bytes = out
                    .body
                    .collect()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?
                    .into_bytes();
                Ok(bytes.to_vec())
            })
        })
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<u64, VfsError> {
        let key = Self::normalize_virtual_path(path)?;
        let handle = self.handle.clone();
        let bucket = self.bucket.clone();
        let content_vec = content.to_vec();
        let len = content_vec.len();
        let client = self.client.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let threshold = 5 * 1024 * 1024;
                if len <= threshold {
                    client
                        .put_object()
                        .bucket(&bucket)
                        .key(&key)
                        .body(ByteStream::from(content_vec))
                        .send()
                        .await
                        .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                } else {
                    let create_res = client
                        .create_multipart_upload()
                        .bucket(&bucket)
                        .key(&key)
                        .send()
                        .await
                        .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                    let upload_id = create_res.upload_id().unwrap_or_default().to_string();
                    let mut completed_parts = Vec::new();

                    for (i, chunk) in content_vec.chunks(threshold).enumerate() {
                        let part_number = i32::try_from(i + 1).unwrap_or(1);
                        let upload_res = self
                            .client
                            .upload_part()
                            .bucket(&bucket)
                            .key(&key)
                            .upload_id(&upload_id)
                            .part_number(part_number)
                            .body(ByteStream::from(chunk.to_vec()))
                            .send()
                            .await
                            .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

                        completed_parts.push(
                            aws_sdk_s3::types::CompletedPart::builder()
                                .e_tag(upload_res.e_tag().unwrap_or_default())
                                .part_number(part_number)
                                .build(),
                        );
                    }

                    let completed_upload = aws_sdk_s3::types::CompletedMultipartUpload::builder()
                        .set_parts(Some(completed_parts))
                        .build();

                    self.client
                        .complete_multipart_upload()
                        .bucket(&bucket)
                        .key(&key)
                        .upload_id(&upload_id)
                        .multipart_upload(completed_upload)
                        .send()
                        .await
                        .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;
                }
                Ok(len as u64)
            })
        })
    }

    fn chmod(&self, _path: &str, _mode: u32) -> Result<(), VfsError> {
        Err(VfsError::Io(std::io::Error::other("Not Supported")))
    }

    fn search(
        &self,
        _path: &str,
        _query: &str,
        _maximum_results: usize,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        Err(VfsError::Io(std::io::Error::other("Not Supported")))
    }
}
