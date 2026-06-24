use std::time::Duration;
use serde_json::Value;
use regex::Regex;
use crate::config::Config;
use crate::types::Metadata;

pub fn strip_to_json(s: &str) -> &str {
    if let Some(p) = s.find(|c| c == '{' || c == '[') {
        &s[p..]
    } else {
        s
    }
}

pub fn sanitize_filename(name: &str) -> String {
    let s = name.replace(' ', "_");
    let re = Regex::new(r"[^a-zA-Z0-9_\-]").unwrap();
    re.replace_all(&s, "").into_owned()
}

pub fn is_show_type(media_type: &str) -> bool {
    let re = Regex::new(r"(?i)show|series|tv|mini|episode|special").unwrap();
    re.is_match(media_type)
}

pub fn fetch_url(url: &str, api_key: &str) -> Result<String, reqwest::Error> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let mut req = client.get(url)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0");

    if !api_key.is_empty() {
        req = req.header("x-api-key", api_key);
    }

    req.send()?.text()
}

pub fn fetch_ani_api(pathname: &str, config: &Config) -> Result<Value, String> {
    let resp = fetch_url(&format!("{}{}", config.base_url, pathname), &config.api_key)
        .map_err(|e| e.to_string())?;

    let cleaned = strip_to_json(&resp);
    serde_json::from_str(cleaned).map_err(|e| format!("JSON Parse failed: {} | Resp: {}", e, resp))
}

pub fn fetch_imdb_metadata(config: &Config) -> Metadata {
    let mut path = format!("/info?provider={}&id={}", config.provider, config.id);
    for arg in &config.args {
        path.push_str(&format!("&args={}", arg));
    }
    let default_title = format!("{}:{}", config.provider, config.id);
    match fetch_ani_api(&path, config) {
        Ok(d) => {
            if d.get("error").is_some() && !d["error"].is_null() {
                println!("[Meta] AniAPI error: {}", d["error"].as_str().unwrap_or("Unknown"));
            }
            serde_json::from_value(d).unwrap_or_else(|_| {
                Metadata {
                    title: default_title.clone(),
                    original_title: None,
                    media_type: Some("movie".to_string()),
                    type_field: None,
                    year: None,
                    episodes: None,
                    has_primary_stream: Some(false),
                }
            })
        }
        Err(e) => {
            eprintln!("[Meta] AniAPI lookup failed: {}", e);
            Metadata {
                title: default_title,
                original_title: None,
                media_type: Some("movie".to_string()),
                type_field: None,
                year: None,
                episodes: None,
                has_primary_stream: Some(false),
            }
        }
    }
}
