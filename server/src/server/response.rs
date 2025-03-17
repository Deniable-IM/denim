use axum::response::{IntoResponse, Json, Response};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageResponse {
    pub needs_sync: bool,
}

impl IntoResponse for SendMessageResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkDeviceToken {
    pub verification_code: String,
    pub token_identifier: String,
}

impl IntoResponse for LinkDeviceToken {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkDeviceResponse {
    pub aci: String,
    pub pni: String,
    pub device_id: u32,
}

impl IntoResponse for LinkDeviceResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}
