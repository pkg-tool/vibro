use std::error::Error;
use std::sync::{LazyLock, OnceLock};
use std::{borrow::Cow, mem, pin::Pin, task::Poll, time::Duration};

use anyhow::anyhow;
use bytes::{BufMut, Bytes, BytesMut};
use futures::{AsyncRead, FutureExt as _, TryStreamExt as _};
use http_client::{RedirectPolicy, Url, http};
use regex::Regex;
use reqwest::{
    header::{HeaderMap, HeaderValue},
    redirect,
};

const DEFAULT_CAPACITY: usize = 4096;
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static REDACT_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"key=[^&]+").unwrap());

pub struct ReqwestClient {
    client: reqwest::Client,
    proxy: Option<Url>,
    user_agent: Option<HeaderValue>,
    handle: tokio::runtime::Handle,
}

impl ReqwestClient {
    fn builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(Duration::from_secs(10))
            .redirect_policy(redirect::Policy::none())
    }

    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("Failed to initialize HTTP client")
            .into()
    }

    pub fn user_agent(agent: &str) -> anyhow::Result<Self> {
        let mut map = HeaderMap::new();
        map.insert(http::header::USER_AGENT, HeaderValue::from_str(agent)?);
        let client = Self::builder().default_headers(map).build()?;
        Ok(client.into())
    }

    pub fn proxy_and_user_agent(proxy: Option<Url>, user_agent: &str) -> anyhow::Result<Self> {
        let user_agent = HeaderValue::from_str(user_agent)?;

        let mut map = HeaderMap::new();
        map.insert(http::header::USER_AGENT, user_agent.clone());
        let mut client = Self::builder().default_headers(map);
        let client_has_proxy;

        if let Some(proxy) = proxy.as_ref().and_then(|proxy_url| {
            reqwest::Proxy::all(proxy_url.clone())
                .inspect_err(|e| {
                    log::error!(
                        "Failed to parse proxy URL '{}': {}",
                        proxy_url,
                        e.source().unwrap_or(&e as &_)
                    )
                })
                .ok()
        }) {
            // Respect NO_PROXY env var
            client = client.proxy(proxy.no_proxy(reqwest::NoProxy::from_env()));
            client_has_proxy = true;
        } else {
            client_has_proxy = false;
        };

        let client = client
            .use_preconfigured_tls(http_client_tls::tls_config())
            .build()?;
        let mut client: ReqwestClient = client.into();
        client.proxy = client_has_proxy.then_some(proxy).flatten();
        client.user_agent = Some(user_agent);
        Ok(client)
    }
}

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            // Since we now have two executors, let's try to keep our footprint small
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("Failed to initialize HTTP client")
    })
}

impl From<reqwest::Client> for ReqwestClient {
    fn from(client: reqwest::Client) -> Self {
        let handle = tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
            log::debug!("no tokio runtime found, creating one for Reqwest...");
            runtime().handle().clone()
        });
        Self {
            client,
            handle,
            proxy: None,
            user_agent: None,
        }
    }
}

// This struct is essentially a re-implementation of
// https://docs.rs/tokio-util/0.7.12/tokio_util/io/struct.ReaderStream.html
// except outside of Tokio's aegis
struct StreamReader {
    reader: Option<Pin<Box<dyn futures::AsyncRead + Send + Sync>>>,
    buf: BytesMut,
    capacity: usize,
}

impl StreamReader {
    fn new(reader: Pin<Box<dyn futures::AsyncRead + Send + Sync>>) -> Self {
        Self {
            reader: Some(reader),
            buf: BytesMut::new(),
            capacity: DEFAULT_CAPACITY,
        }
    }
}

impl futures::Stream for StreamReader {
    type Item = std::io::Result<Bytes>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut();

        let mut reader = match this.reader.take() {
            Some(r) => r,
            None => return Poll::Ready(None),
        };

        if this.buf.capacity() == 0 {
            let capacity = this.capacity;
            this.buf.reserve(capacity);
        }

        match poll_read_buf(&mut reader, cx, &mut this.buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => {
                self.reader = None;

                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(Ok(0)) => {
                self.reader = None;
                Poll::Ready(None)
            }
            Poll::Ready(Ok(_)) => {
                let chunk = this.buf.split();
                self.reader = Some(reader);
                Poll::Ready(Some(Ok(chunk.freeze())))
            }
        }
    }
}

/// Implementation from <https://docs.rs/tokio-util/0.7.12/src/tokio_util/util/poll_buf.rs.html>
/// Specialized for this use case
pub fn poll_read_buf(
    io: &mut Pin<Box<dyn futures::AsyncRead + Send + Sync>>,
    cx: &mut std::task::Context<'_>,
    buf: &mut BytesMut,
) -> Poll<std::io::Result<usize>> {
    if !buf.has_remaining_mut() {
        return Poll::Ready(Ok(0));
    }

    let n = {
        let dst = buf.chunk_mut();

        // Safety: `chunk_mut()` returns a `&mut UninitSlice`, and `UninitSlice` is a
        // transparent wrapper around `[MaybeUninit<u8>]`.
        let dst = unsafe { &mut *(dst as *mut _ as *mut [std::mem::MaybeUninit<u8>]) };
        let mut buf = tokio::io::ReadBuf::uninit(dst);
        let ptr = buf.filled().as_ptr();
        let unfilled_portion = buf.initialize_unfilled();
        // SAFETY: Pin projection
        let io_pin = unsafe { Pin::new_unchecked(io) };
        std::task::ready!(io_pin.poll_read(cx, unfilled_portion)?);

        // Ensure the pointer does not change from under us
        assert_eq!(ptr, buf.filled().as_ptr());
        buf.filled().len()
    };

    // Safety: This is guaranteed to be the number of initialized (and read)
    // bytes due to the invariants provided by `ReadBuf::filled`.
    unsafe {
        buf.advance_mut(n);
    }

    Poll::Ready(Ok(n))
}

fn redact_error(mut error: reqwest::Error) -> reqwest::Error {
    if let Some(url) = error.url_mut()
        && let Some(query) = url.query()
        && let Cow::Owned(redacted) = REDACT_REGEX.replace_all(query, "key=REDACTED")
    {
        url.set_query(Some(redacted.as_str()));
    }
    error
}

impl http_client::HttpClient for ReqwestClient {
    fn proxy(&self) -> Option<&Url> {
        self.proxy.as_ref()
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        self.user_agent.as_ref()
    }

    fn send(
        &self,
        req: http::Request<http_client::AsyncBody>,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<http_client::Response<http_client::AsyncBody>>,
    > {
        let (parts, body) = req.into_parts();

        let method = parts.method;
        let mut headers = parts.headers;
        let redirect_policy = parts
            .extensions
            .get::<RedirectPolicy>()
            .cloned()
            .unwrap_or(RedirectPolicy::NoFollow);

        enum Body {
            Empty,
            Bytes(Bytes),
            Stream(Pin<Box<dyn futures::AsyncRead + Send + Sync>>),
        }

        let body = match body.0 {
            http_client::Inner::Empty => Body::Empty,
            http_client::Inner::Bytes(cursor) => Body::Bytes(cursor.into_inner()),
            http_client::Inner::AsyncReader(stream) => Body::Stream(stream),
        };

        let handle = self.handle.clone();
        let client = self.client.clone();
        async move {
            let mut redirects_remaining = match redirect_policy {
                RedirectPolicy::NoFollow => 0,
                RedirectPolicy::FollowLimit(limit) => limit as usize,
                RedirectPolicy::FollowAll => 100,
            };

            let mut current_url = Url::parse(&parts.uri.to_string())
                .map_err(|e| anyhow!("invalid url {:?}: {e}", parts.uri.to_string()))?;

            let (stream, bytes) = match body {
                Body::Empty => (None, None),
                Body::Bytes(bytes) => (None, Some(bytes)),
                Body::Stream(stream) => (Some(stream), None),
            };

            let mut response = if let Some(stream) = stream {
                if redirects_remaining > 0 {
                    return Err(anyhow!(
                        "cannot follow redirects for streaming request bodies"
                    ));
                }

                let request = client
                    .request(method.clone(), current_url.to_string())
                    .headers(headers.clone())
                    .body(reqwest::Body::wrap_stream(StreamReader::new(stream)));

                handle
                    .spawn(async { request.send().await })
                    .await?
                    .map_err(redact_error)?
            } else {
                loop {
                    let mut request = client
                        .request(method.clone(), current_url.to_string())
                        .headers(headers.clone());

                    request = match bytes.as_ref() {
                        Some(bytes) => request.body(bytes.clone()),
                        None => request.body(reqwest::Body::default()),
                    };

                    let response = handle
                        .spawn(async { request.send().await })
                        .await?
                        .map_err(redact_error)?;

                    if redirects_remaining == 0 || !response.status().is_redirection() {
                        break response;
                    }

                    let location = response
                        .headers()
                        .get(http::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| anyhow!("missing or invalid Location header"))?;

                    let next_url = match Url::parse(location) {
                        Ok(url) => url,
                        Err(_) => current_url.join(location)?,
                    };

                    let origin_changed = current_url.scheme() != next_url.scheme()
                        || current_url.host_str() != next_url.host_str()
                        || current_url.port_or_known_default() != next_url.port_or_known_default();

                    if origin_changed {
                        headers.remove(http::header::AUTHORIZATION);
                        headers.remove(http::header::COOKIE);
                    }

                    current_url = next_url;
                    redirects_remaining -= 1;
                }
            };

            let headers = mem::take(response.headers_mut());
            let mut builder = http::Response::builder()
                .status(response.status().as_u16())
                .version(response.version());
            *builder.headers_mut().unwrap() = headers;

            let bytes = response
                .bytes_stream()
                .map_err(futures::io::Error::other)
                .into_async_read();
            let body = http_client::AsyncBody::from_reader(bytes);

            builder.body(body).map_err(|e| anyhow!(e))
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use http_client::{HttpClient, Url};

    use crate::ReqwestClient;

    #[test]
    fn test_proxy_uri() {
        let client = ReqwestClient::new();
        assert_eq!(client.proxy(), None);

        let proxy = Url::parse("http://localhost:10809").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy.clone()), "test").unwrap();
        assert_eq!(client.proxy(), Some(&proxy));

        let proxy = Url::parse("https://localhost:10809").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy.clone()), "test").unwrap();
        assert_eq!(client.proxy(), Some(&proxy));

        let proxy = Url::parse("socks4://localhost:10808").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy.clone()), "test").unwrap();
        assert_eq!(client.proxy(), Some(&proxy));

        let proxy = Url::parse("socks4a://localhost:10808").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy.clone()), "test").unwrap();
        assert_eq!(client.proxy(), Some(&proxy));

        let proxy = Url::parse("socks5://localhost:10808").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy.clone()), "test").unwrap();
        assert_eq!(client.proxy(), Some(&proxy));

        let proxy = Url::parse("socks5h://localhost:10808").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy.clone()), "test").unwrap();
        assert_eq!(client.proxy(), Some(&proxy));
    }

    #[test]
    fn test_invalid_proxy_uri() {
        let proxy = Url::parse("socks://127.0.0.1:20170").unwrap();
        let client = ReqwestClient::proxy_and_user_agent(Some(proxy), "test").unwrap();
        assert!(
            client.proxy.is_none(),
            "An invalid proxy URL should add no proxy to the client!"
        )
    }
}
