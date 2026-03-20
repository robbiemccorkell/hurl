const BUILTIN_GITHUB_CLIENT_ID: &str = "Ov23liRAvOyZUSYd005n";

pub fn github_client_id() -> Option<String> {
    let value = BUILTIN_GITHUB_CLIENT_ID.trim();
    (!value.is_empty()).then(|| value.to_string())
}
