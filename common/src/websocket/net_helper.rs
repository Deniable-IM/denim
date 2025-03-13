use crate::{
    signalservice::{
        web_socket_message, WebSocketMessage, WebSocketRequestMessage, WebSocketResponseMessage,
    },
    web_api::DenimMessages,
};
use axum::http::{StatusCode, Uri};
use rand::{rngs::OsRng, Rng as _};
use std::{
    str::FromStr,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

pub struct PathExtractor {
    parts: Vec<String>,
}

// rust does not have variadic templates so therefore we must cope, and extract them one by one
impl PathExtractor {
    pub fn new(uri: &Uri) -> Result<Self, String> {
        let uri = uri.path();

        let mut extractor = Self {
            parts: uri.split("/").map(String::from).collect(),
        };
        if extractor.parts.is_empty() {
            return Err("PathExtractor: Is empty!".to_string());
        }
        if uri.starts_with("/") {
            extractor.parts.remove(0);
        }
        Ok(extractor)
    }
    pub fn extract<T: FromStr<Err = impl std::fmt::Debug>>(
        &self,
        index: usize,
    ) -> Result<T, String> {
        if index >= self.parts.len() {
            return Err("PathExtractor: Larger than count".to_string());
        }
        T::from_str(&self.parts[index]).map_err(|_| "failed to convert".to_string())
    }
}

pub fn create_response(
    id: u64,
    status_code: StatusCode,
    mut headers: Vec<String>,
    body: Option<Vec<u8>>,
) -> Result<WebSocketMessage, String> {
    if !headers.iter().any(|x| x.starts_with("Content-Length")) {
        headers.push(format!(
            "Content-Length: {}",
            body.as_ref().map(|v| v.len()).unwrap_or(0)
        ));
    }

    let res = WebSocketResponseMessage {
        id: Some(id),
        status: Some(status_code.as_u16() as u32),
        message: Some(
            status_code
                .canonical_reason()
                .ok_or("Invalid canonical reason")?
                .to_string(),
        ),
        headers,
        body,
    };

    Ok(WebSocketMessage {
        r#type: Some(web_socket_message::Type::Response as i32),
        request: None,
        response: Some(res),
    })
}

pub fn create_request(
    id: u64,
    verb: &str,
    path: &str,
    headers: Vec<String>,
    body: Option<Vec<u8>>,
) -> WebSocketMessage {
    let req = WebSocketRequestMessage {
        verb: Some(verb.to_string()),
        path: Some(path.to_string()),
        body,
        headers,
        id: Some(id),
    };
    WebSocketMessage {
        r#type: Some(web_socket_message::Type::Request as i32),
        request: Some(req),
        response: None,
    }
}

pub fn unpack_messages(body: Option<Vec<u8>>) -> Result<DenimMessages, String> {
    let json = String::from_utf8(body.ok_or_else(|| "Body was none".to_string())?)
        .map_err(|_| "Failed to convert req body to string".to_string())?;

    serde_json::from_str(&json).map_err(|_| "Failed to convert json to DenimMessages".to_string())
}

pub fn generate_req_id() -> u64 {
    let mut rng = OsRng;
    let rand_v: u64 = rng.gen();
    rand_v
}

pub fn current_millis() -> Result<u128, SystemTimeError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
}

#[cfg(test)]
mod test {
    use super::{create_request, create_response, unpack_messages, PathExtractor};
    use crate::{signalservice::web_socket_message, web_api::RegularPayload};
    use axum::http::{StatusCode, Uri};
    use std::str::FromStr;

    #[test]
    fn test_path_extractor() {
        let uri = Uri::from_str("/a/b/1/true/hello").unwrap();
        let extractor = PathExtractor::new(&uri).unwrap();

        let (x, y, z) = extractor
            .extract::<u8>(2)
            .and_then(|x| extractor.extract::<bool>(3).map(|y| (x, y)))
            .and_then(|(x, y)| extractor.extract::<String>(4).map(|z| (x, y, z)))
            .expect("Expected that 2, 3, 4 were int, bool and string");

        assert!(x == 1);
        assert!(y);
        assert!(z == "hello");
    }

    #[test]
    fn test_create_response() {
        let msg =
            create_response(1, StatusCode::OK, vec!["my-header: ok".to_string()], None).unwrap();
        assert!(msg.r#type() == web_socket_message::Type::Response);
        let res = msg.response.unwrap();
        assert!(res.id() == 1);
        assert!(res.message() == "OK");
        assert!(res.status() == 200);
        assert!(res.body.is_none());
        assert!(res.headers[0] == "my-header: ok");
    }

    #[test]
    fn test_create_request() {
        let msg = create_request(
            1,
            "PUT",
            "/v1/messages",
            vec!["my-header: ok".to_string()],
            None,
        );
        assert!(msg.r#type() == web_socket_message::Type::Request);
        let req = msg.request.unwrap();
        assert!(req.id() == 1);
        assert!(req.verb() == "PUT");
        assert!(req.path() == "/v1/messages");
        assert!(req.body.is_none());
        assert!(req.headers[0] == "my-header: ok");
    }

    #[test]
    fn test_unpack_messages() {
        let msg = r#"
        {
            "messages":[
                {
                    "regularPayload": {
                        "signalMessage": {
                            "type": 1,
                            "destinationDeviceId": 3,
                            "destinationRegistrationId": 22,
                            "content": "aGVsbG8="
                        }
                    },
                    "chunks": [],
                    "ballast": 0
                }
            ],
            "online": false,
            "urgent": true,
            "timestamp": 1730217386
        }
        "#;
        let b = msg.as_bytes().to_vec();

        let req = create_request(1, "PUT", "/v1/messages", vec![], Some(b));

        let msg = unpack_messages(req.request.unwrap().body).unwrap();
        assert!(!msg.online);
        assert!(msg.urgent);
        assert!(msg.timestamp == 1730217386);
        let RegularPayload::SignalMessage(signal_msg) = msg.messages[0].regular_payload.clone()
        else {
            panic!()
        };
        assert!(signal_msg.content == "aGVsbG8=");
        assert!(signal_msg.r#type == 1);
        assert!(signal_msg.destination_device_id == 3);
        assert!(signal_msg.destination_registration_id == 22);
    }
}
