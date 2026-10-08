//! A blocking PocketBase REST client.

use crate::contract::{ChangeEntry, CommandOutcome, CommandReply, EditRecord, RecordSnapshot};
use crate::{CollabError, CollabResult};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

/// Records fetched per page when listing.
const PAGE_SIZE: usize = 200;

/// One HTTP exchange: the status and the body as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: String,
    /// The `X-Collab-Replayed` header was `true`.
    pub replayed: bool,
}

impl HttpReply {
    /// Parses the body as JSON if the status is 2xx; maps 401 and 403 to
    /// their own errors and anything else to [`CollabError::Http`].
    pub fn json<T: DeserializeOwned>(&self) -> CollabResult<T> {
        self.success()?;
        serde_json::from_str(&self.body)
            .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", self.body)))
    }

    /// Ok for 2xx statuses, the matching error otherwise.
    pub fn success(&self) -> CollabResult<()> {
        let message = || {
            serde_json::from_str::<Value>(&self.body)
                .ok()
                .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
                .unwrap_or_else(|| self.body.clone())
        };
        match self.status {
            200..=299 => Ok(()),
            401 => Err(CollabError::Unauthorized(message())),
            403 => Err(CollabError::Forbidden(message())),
            status => Err(CollabError::Http {
                status,
                message: message(),
            }),
        }
    }
}

/// A PocketBase API client, optionally signed in.
#[derive(Clone)]
pub struct PocketBaseClient {
    base_url: String,
    agent: ureq::Agent,
    token: Option<String>,
    auth_id: Option<String>,
}

impl std::fmt::Debug for PocketBaseClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PocketBaseClient")
            .field("base_url", &self.base_url)
            .field("auth_id", &self.auth_id)
            .finish_non_exhaustive()
    }
}

impl PocketBaseClient {
    /// An anonymous client for `base_url`, e.g. `http://127.0.0.1:8090`.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(60))
                .build(),
            token: None,
            auth_id: None,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The auth token, if signed in.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// The signed-in record's ID, if signed in.
    pub fn auth_id(&self) -> Option<&str> {
        self.auth_id.as_deref()
    }

    /// Signs in to an auth collection (`users`, or `_superusers`) and
    /// returns a client that sends the token.
    pub fn authenticate(
        &self,
        collection: &str,
        identity: &str,
        password: &str,
    ) -> CollabResult<Self> {
        let reply = self.post_json(
            &format!("/api/collections/{collection}/auth-with-password"),
            &serde_json::json!({ "identity": identity, "password": password }),
        )?;
        let auth: Value = reply.json()?;
        let token = auth["token"]
            .as_str()
            .ok_or_else(|| CollabError::Protocol("the auth reply has no token".into()))?;
        let id = auth["record"]["id"]
            .as_str()
            .ok_or_else(|| CollabError::Protocol("the auth reply has no record ID".into()))?;
        Ok(Self {
            token: Some(token.to_owned()),
            auth_id: Some(id.to_owned()),
            ..self.clone()
        })
    }

    /// Sends a protected edit command. Applied, no-op, conflict and
    /// correlation-reuse answers are all `Ok`; inspect the outcome.
    pub fn execute(&self, command: &EditRecord) -> CollabResult<CommandReply> {
        let body = serde_json::to_value(command)
            .map_err(|error| CollabError::Protocol(error.to_string()))?;
        let reply = self.post_json("/api/collab/commands", &body)?;
        match reply.status {
            200 | 409 | 422 => {
                let outcome: CommandOutcome = serde_json::from_str(&reply.body)
                    .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
                Ok(CommandReply {
                    status: reply.status,
                    outcome,
                    replayed: reply.replayed,
                    body: reply.body,
                })
            }
            _ => Err(reply.success().err().unwrap_or_else(|| {
                CollabError::Protocol(format!("unexpected status {}", reply.status))
            })),
        }
    }

    /// Every change after `checkpoint`, in sequence order.
    pub fn changes_since(&self, checkpoint: u64) -> CollabResult<Vec<ChangeEntry>> {
        self.list_records("collab_changes", &format!("seq > {checkpoint}"), "seq")
    }

    /// The record with `key`, if there is one.
    pub fn record_by_key(&self, key: &str) -> CollabResult<Option<RecordSnapshot>> {
        let filter = format!("key = {}", serde_json::Value::from(key));
        Ok(self
            .list_records("collab_records", &filter, "key")?
            .into_iter()
            .next())
    }

    /// Every record of `collection` matching `filter` (may be empty), in
    /// `sort` order, across all pages.
    pub fn list_records<T: DeserializeOwned>(
        &self,
        collection: &str,
        filter: &str,
        sort: &str,
    ) -> CollabResult<Vec<T>> {
        let mut items = Vec::new();
        for page in 1.. {
            let query = [
                ("page", page.to_string()),
                ("perPage", PAGE_SIZE.to_string()),
                ("sort", sort.to_owned()),
                ("filter", filter.to_owned()),
                ("skipTotal", "1".to_owned()),
            ];
            let reply =
                self.get_with_query(&format!("/api/collections/{collection}/records"), &query)?;
            let mut list: Value = reply.json()?;
            let batch = list["items"]
                .as_array_mut()
                .ok_or_else(|| CollabError::Protocol("a list reply has no items".into()))?;
            let count = batch.len();
            for item in batch.drain(..) {
                items.push(
                    serde_json::from_value(item)
                        .map_err(|error| CollabError::Protocol(error.to_string()))?,
                );
            }
            if count < PAGE_SIZE {
                break;
            }
        }
        Ok(items)
    }

    /// Creates a record with one uploaded file (multipart/form-data).
    pub fn create_with_file(
        &self,
        collection: &str,
        fields: &[(&str, &str)],
        file_field: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> CollabResult<Value> {
        let boundary = format!("collab-{}", crate::CorrelationId::generate());
        let mut body = Vec::new();
        for (name, value) in fields {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let request = self
            .request("POST", &format!("/api/collections/{collection}/records"))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            );
        self.send(request, Some(&body))?.json()
    }

    /// A short-lived token for downloading protected files.
    pub fn file_token(&self) -> CollabResult<String> {
        let reply = self.post_json("/api/files/token", &serde_json::json!({}))?;
        let value: Value = reply.json()?;
        value["token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| CollabError::Protocol("the file token reply has no token".into()))
    }

    /// The bytes of a stored file; `token` is needed for protected files.
    pub fn download_file(
        &self,
        collection: &str,
        record_id: &str,
        file_name: &str,
        token: Option<&str>,
    ) -> CollabResult<Vec<u8>> {
        let mut request = self.request(
            "GET",
            &format!("/api/files/{collection}/{record_id}/{file_name}"),
        );
        if let Some(token) = token {
            request = request.query("token", token);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let body = response.into_string().unwrap_or_default();
                return Err(HttpReply {
                    status,
                    body,
                    replayed: false,
                }
                .success()
                .err()
                .unwrap_or_else(|| CollabError::Protocol("unexpected status".into())));
            }
            Err(transport) => return Err(CollabError::Transport(transport.to_string())),
        };
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|io| CollabError::Transport(io.to_string()))?;
        Ok(bytes)
    }

    pub fn get(&self, path: &str) -> CollabResult<HttpReply> {
        self.send(self.request("GET", path), None)
    }

    pub fn get_with_query(&self, path: &str, query: &[(&str, String)]) -> CollabResult<HttpReply> {
        let mut request = self.request("GET", path);
        for (name, value) in query {
            if !value.is_empty() {
                request = request.query(name, value);
            }
        }
        self.send(request, None)
    }

    pub fn post_json(&self, path: &str, body: &Value) -> CollabResult<HttpReply> {
        self.send_json("POST", path, body)
    }

    pub fn patch_json(&self, path: &str, body: &Value) -> CollabResult<HttpReply> {
        self.send_json("PATCH", path, body)
    }

    fn send_json(&self, method: &str, path: &str, body: &Value) -> CollabResult<HttpReply> {
        let request = self
            .request(method, path)
            .set("Content-Type", "application/json");
        self.send(request, Some(body.to_string().as_bytes()))
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        let request = self
            .agent
            .request(method, &format!("{}{path}", self.base_url));
        match &self.token {
            Some(token) => request.set("Authorization", token),
            None => request,
        }
    }

    fn send(&self, request: ureq::Request, body: Option<&[u8]>) -> CollabResult<HttpReply> {
        let result = match body {
            Some(bytes) => request.send_bytes(bytes),
            None => request.call(),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(transport) => return Err(CollabError::Transport(transport.to_string())),
        };
        let status = response.status();
        let replayed = response.header("X-Collab-Replayed") == Some("true");
        let body = response
            .into_string()
            .map_err(|io| CollabError::Transport(io.to_string()))?;
        Ok(HttpReply {
            status,
            body,
            replayed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_errors() {
        let reply = |status, body: &str| HttpReply {
            status,
            body: body.into(),
            replayed: false,
        };
        assert!(reply(204, "").success().is_ok());
        assert_eq!(
            reply(401, r#"{"message":"no token"}"#).success(),
            Err(CollabError::Unauthorized("no token".into()))
        );
        assert_eq!(
            reply(403, r#"{"message":"rules"}"#).success(),
            Err(CollabError::Forbidden("rules".into()))
        );
        assert_eq!(
            reply(500, "boom").success(),
            Err(CollabError::Http {
                status: 500,
                message: "boom".into()
            })
        );
        assert!(reply(200, "not json").json::<Value>().is_err());
    }

    #[test]
    fn unreachable_servers_are_transport_errors() {
        // Port 9 (discard) on loopback is closed on any normal machine.
        let client = PocketBaseClient::new("http://127.0.0.1:9/");
        assert_eq!(client.base_url(), "http://127.0.0.1:9");
        assert!(matches!(
            client.get("/api/health"),
            Err(CollabError::Transport(_))
        ));
    }
}
