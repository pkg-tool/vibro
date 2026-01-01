use gpui::{AsyncApp, actions};

const VECTOR_URL_SCHEME: &str = "vector";

actions!(
    cli,
    [
        /// Registers the vector:// URL scheme handler.
        RegisterZedScheme
    ]
);

pub async fn register_zed_scheme(cx: &AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| cx.register_url_scheme(VECTOR_URL_SCHEME))?
        .await
}
