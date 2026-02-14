use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};
use reqwest::Client;

const APP_USER_AGENT: &str = "InterfaceOficial/0.1.0";

pub fn build_http_client() -> Result<Client, reqwest::Error> {
    let mut default_headers = HeaderMap::new();
    default_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

    Client::builder()
        .user_agent(APP_USER_AGENT)
        .default_headers(default_headers)
        .build()
}
