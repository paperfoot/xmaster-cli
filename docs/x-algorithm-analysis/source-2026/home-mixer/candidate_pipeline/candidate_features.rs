use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PureCoreData {
    pub author_id: u64,
    pub text: String,
    pub source_tweet_id: Option<u64>,
    pub source_user_id: Option<u64>,
    pub in_reply_to_tweet_id: Option<u64>,
    pub in_reply_to_user_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExclusiveTweetControl {
    pub conversation_author_id: i64,
}

pub type MediaEntities = Vec<MediaEntity>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaEntity {
    pub media_info: Option<MediaInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MediaInfo {
    VideoInfo(VideoInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub duration_millis: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub source_tweet_id: u64,
    pub source_user_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub in_reply_to_tweet_id: Option<u64>,
    pub in_reply_to_user_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GizmoduckUserCounts {
    pub followers_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GizmoduckUserProfile {
    pub screen_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GizmoduckUser {
    pub user_id: u64,
    pub profile: GizmoduckUserProfile,
    pub counts: GizmoduckUserCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GizmoduckUserResult {
    pub user: Option<GizmoduckUser>,
}
