#[derive(Debug, Clone)]
pub struct OAuthCredentials {
    pub refresh: String,
    pub access: String,
    pub expires: i64,
    pub account_id: Option<String>,
    pub enterprise_url: Option<String>,
}
