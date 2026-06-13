use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Task {
    pub season: String,
    pub episode: usize,
    pub base_dir: String,
    pub file_name_base: String,
    pub imdb_id: String,
    pub sub_url: String,
    pub downloaded: bool,
    pub failed: bool,
    pub failure_printed: bool,
    pub claimed_by: usize,
}

pub struct WorkerStatus {
    pub id: usize,
    pub status: String,
    pub progress: f64,
    pub current_task: Option<Task>,
    pub last_output: String,
}

pub struct DownloadManager {
    pub tasks: Vec<Task>,
    pub workers: Vec<WorkerStatus>,
    pub is_bulk: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Metadata {
    pub title: String,
    #[serde(rename = "originalTitle")]
    pub original_title: Option<String>,
    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,
    #[serde(rename = "type")]
    pub type_field: Option<String>,
    pub year: Option<i32>,
    pub episodes: Option<Value>,
    #[serde(rename = "hasPrimaryStream")]
    pub has_primary_stream: Option<bool>,
}
