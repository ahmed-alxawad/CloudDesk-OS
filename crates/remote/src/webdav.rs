use std::path::{Component, Path, PathBuf};

use clouddesk_vfs::{EntryKind, ProviderFeature, VfsEntry, VfsError, VfsProvider};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::{Client, Method};
use tokio::runtime::Handle;

pub struct WebDavProvider {
    client: Client,
    base_url: String,
    username: Option<String>,
    password: Option<String>,
    handle: Handle,
}

impl WebDavProvider {
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(
        base_url: String,
        username: Option<String>,
        password: Option<String>,
        handle: Handle,
    ) -> Self {
        let client = Client::builder().build().unwrap_or_else(|_| Client::new());
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            username,
            password,
            handle,
        }
    }

    fn normalize_virtual_path(path: &str) -> Result<PathBuf, VfsError> {
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
        Ok(normalized)
    }

    fn url_for(&self, path: &str) -> Result<String, VfsError> {
        let normalized = Self::normalize_virtual_path(path)?;
        let path_str = normalized.to_str().unwrap_or("");
        let url_path = urlencoding::encode(path_str).replace("%2F", "/");
        if url_path.is_empty() {
            Ok(self.base_url.clone())
        } else {
            Ok(format!("{}/{}", self.base_url, url_path))
        }
    }

    async fn execute_request(
        &self,
        method: Method,
        path: &str,
        depth: Option<&str>,
        body: Option<Vec<u8>>,
        extra_headers: Vec<(&str, &str)>,
    ) -> Result<reqwest::Response, VfsError> {
        let url = self.url_for(path)?;
        let mut req = self.client.request(method.clone(), &url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req = req.basic_auth(u, Some(p));
        }
        if let Some(d) = depth {
            req = req.header("Depth", d);
        }
        for (k, v) in extra_headers {
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req.body(b);
        }

        let res = req
            .send()
            .await
            .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?;

        // Let GET/PUT pass their specific statuses back, but for WebDAV operations check if failed
        if !res.status().is_success() && method != Method::GET {
            if res.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(VfsError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Not Found",
                )));
            }
            return Err(VfsError::Io(std::io::Error::other(format!(
                "HTTP Error: {}",
                res.status()
            ))));
        }
        Ok(res)
    }

    fn parse_propfind(xml: &str, base_path: &str) -> Vec<VfsEntry> {
        let mut entries = Vec::new();

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut in_response = false;
        let mut current_href = String::new();
        let mut current_size = 0;
        let mut is_collection = false;
        let mut current_tag = String::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let tag = String::from_utf8_lossy(name.as_ref()).to_lowercase();
                    let short_tag = tag.split(':').next_back().unwrap_or(&tag);
                    current_tag = short_tag.to_string();

                    if current_tag == "response" {
                        in_response = true;
                        current_href.clear();
                        current_size = 0;
                        is_collection = false;
                    } else if current_tag == "collection" {
                        is_collection = true;
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = e.unescape().unwrap_or_default().into_owned();
                    if in_response {
                        if current_tag == "href" {
                            current_href = text;
                        } else if current_tag == "getcontentlength" {
                            current_size = text.parse().unwrap_or(0);
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = e.name();
                    let tag = String::from_utf8_lossy(name.as_ref()).to_lowercase();
                    let short_tag = tag.split(':').next_back().unwrap_or(&tag);

                    if short_tag == "response" {
                        in_response = false;

                        let decoded_href = urlencoding::decode(&current_href)
                            .unwrap_or_else(|_| std::borrow::Cow::Borrowed(&current_href))
                            .into_owned();
                        let decoded_href = decoded_href.trim_end_matches('/');
                        let name = decoded_href
                            .split('/')
                            .next_back()
                            .unwrap_or("")
                            .to_string();

                        if !name.is_empty() {
                            let kind = if is_collection {
                                EntryKind::Directory
                            } else {
                                EntryKind::File
                            };
                            let vfs_path =
                                if base_path == "." || base_path.is_empty() || base_path == "/" {
                                    format!("/{name}")
                                } else {
                                    let base = base_path.trim_start_matches('/');
                                    format!("/{base}/{name}")
                                };

                            entries.push(VfsEntry {
                                name,
                                path: vfs_path,
                                kind,
                                size: current_size,
                                modified_at: None,
                                mode: 0,
                                uid: 0,
                                gid: 0,
                            });
                        }
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => (),
            }
            buf.clear();
        }

        entries
    }
}

impl VfsProvider for WebDavProvider {
    fn capabilities(&self) -> Vec<ProviderFeature> {
        vec![
            ProviderFeature::Read,
            ProviderFeature::Write,
            ProviderFeature::Trash,
        ]
    }

    fn list(&self, path: &str) -> Result<Vec<VfsEntry>, VfsError> {
        let handle = self.handle.clone();
        let path = path.to_string();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let req_body = r#"<?xml version="1.0" encoding="utf-8" ?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/></D:prop></D:propfind>"#;
                let res = self.execute_request(Method::from_bytes(b"PROPFIND").unwrap(), &path, Some("1"), Some(req_body.as_bytes().to_vec()), vec![]).await?;
                let xml = res.text().await.unwrap_or_default();

                let entries = Self::parse_propfind(&xml, &path);
                // Filter out the directory itself from the list
                let normalized = Self::normalize_virtual_path(&path)?;
                let self_name = normalized.file_name().and_then(|n| n.to_str()).unwrap_or("");
                Ok(entries.into_iter().filter(|e| e.name != self_name).collect())
            })
        })
    }

    fn stat(&self, path: &str) -> Result<VfsEntry, VfsError> {
        let handle = self.handle.clone();
        let path = path.to_string();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let req_body = r#"<?xml version="1.0" encoding="utf-8" ?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/></D:prop></D:propfind>"#;
                let res = self.execute_request(Method::from_bytes(b"PROPFIND").unwrap(), &path, Some("0"), Some(req_body.as_bytes().to_vec()), vec![]).await?;
                let xml = res.text().await.unwrap_or_default();
                let mut entries = Self::parse_propfind(&xml, &path);
                entries.pop().ok_or(VfsError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "Not Found")))
            })
        })
    }

    fn create_directory(&self, path: &str) -> Result<(), VfsError> {
        let handle = self.handle.clone();
        let path = path.to_string();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.execute_request(
                    Method::from_bytes(b"MKCOL").unwrap(),
                    &path,
                    None,
                    None,
                    vec![],
                )
                .await?;
                Ok(())
            })
        })
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let handle = self.handle.clone();
        let from = from.to_string();
        let to_url = self.url_for(to)?;
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.execute_request(
                    Method::from_bytes(b"MOVE").unwrap(),
                    &from,
                    None,
                    None,
                    vec![("Destination", &to_url)],
                )
                .await?;
                Ok(())
            })
        })
    }

    fn copy_file(&self, from: &str, to: &str) -> Result<u64, VfsError> {
        let handle = self.handle.clone();
        let from = from.to_string();
        let to_url = self.url_for(to)?;
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.execute_request(
                    Method::from_bytes(b"COPY").unwrap(),
                    &from,
                    None,
                    None,
                    vec![("Destination", &to_url)],
                )
                .await?;
                Ok(0) // Length is unknown strictly
            })
        })
    }

    fn trash(&self, path: &str) -> Result<String, VfsError> {
        let handle = self.handle.clone();
        let path = path.to_string();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.execute_request(Method::DELETE, &path, None, None, vec![])
                    .await?;
                Ok("deleted".to_string())
            })
        })
    }

    fn read_limited(&self, path: &str, maximum_bytes: usize) -> Result<Vec<u8>, VfsError> {
        let handle = self.handle.clone();
        let path = path.to_string();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let res = self
                    .execute_request(Method::GET, &path, None, None, vec![])
                    .await?;
                if !res.status().is_success() {
                    return Err(VfsError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Not Found",
                    )));
                }
                let mut data = res
                    .bytes()
                    .await
                    .map_err(|e| VfsError::Io(std::io::Error::other(e.to_string())))?
                    .to_vec();
                if data.len() > maximum_bytes {
                    data.truncate(maximum_bytes);
                }
                Ok(data)
            })
        })
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<u64, VfsError> {
        let handle = self.handle.clone();
        let path = path.to_string();
        let content = content.to_vec();
        tokio::task::block_in_place(move || {
            handle.block_on(async move {
                self.execute_request(Method::PUT, &path, None, Some(content.clone()), vec![])
                    .await?;
                Ok(content.len() as u64)
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
