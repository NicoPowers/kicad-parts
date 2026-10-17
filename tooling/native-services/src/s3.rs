//! A minimal S3 client (AWS Signature Version 4, path-style) for checking
//! what services wrote to MinIO, independently of the service itself.

use crate::fetch::{hex, sha256_hex};
use crate::{ServiceResult, error};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The region MinIO accepts by default.
pub const REGION: &str = "us-east-1";

/// One object in a bucket listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
}

#[derive(Clone)]
pub struct S3Client {
    endpoint: String,
    host: String,
    access_key: String,
    secret_key: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for S3Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("S3Client")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl S3Client {
    /// `endpoint` is `http://host:port` with no path.
    pub fn new(endpoint: &str, access_key: &str, secret_key: &str) -> Self {
        let endpoint = endpoint.trim_end_matches('/').to_owned();
        let host = endpoint
            .split_once("://")
            .map_or(endpoint.as_str(), |(_, rest)| rest)
            .to_owned();
        Self {
            endpoint,
            host,
            access_key: access_key.to_owned(),
            secret_key: secret_key.to_owned(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(120))
                .build(),
        }
    }

    /// Lists the buckets and ignores the answer. Only a server that knows
    /// these credentials accepts the call, so it proves we reached the
    /// server we started.
    pub fn check_credentials(&self) -> ServiceResult<()> {
        self.send("GET", "/", &[], &[]).map(|_| ())
    }

    pub fn create_bucket(&self, bucket: &str) -> ServiceResult<()> {
        self.send("PUT", &format!("/{bucket}"), &[], &[])
            .map(|_| ())
    }

    pub fn put_object(&self, bucket: &str, key: &str, body: &[u8]) -> ServiceResult<()> {
        self.send("PUT", &object_path(bucket, key), &[], body)
            .map(|_| ())
    }

    pub fn get_object(&self, bucket: &str, key: &str) -> ServiceResult<Vec<u8>> {
        self.send("GET", &object_path(bucket, key), &[], &[])
    }

    /// Every object whose key starts with `prefix`, following continuation
    /// tokens.
    pub fn list_objects(&self, bucket: &str, prefix: &str) -> ServiceResult<Vec<S3Object>> {
        let mut objects = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![
                ("list-type".to_owned(), "2".to_owned()),
                ("prefix".to_owned(), prefix.to_owned()),
            ];
            if let Some(token) = &token {
                query.push(("continuation-token".to_owned(), token.clone()));
            }
            let body = self.send("GET", &format!("/{bucket}"), &query, &[])?;
            let xml = String::from_utf8(body)
                .map_err(|_| error("the bucket listing is not UTF-8".to_owned()))?;
            for contents in elements(&xml, "Contents") {
                let key = element(contents, "Key")
                    .ok_or_else(|| error("a listed object has no Key".to_owned()))?;
                let size = element(contents, "Size")
                    .and_then(|size| size.parse().ok())
                    .ok_or_else(|| error(format!("listed object `{key}` has no Size")))?;
                objects.push(S3Object {
                    key: unescape_xml(key),
                    size,
                });
            }
            if element(&xml, "IsTruncated") == Some("true") {
                token = element(&xml, "NextContinuationToken").map(unescape_xml);
                if token.is_none() {
                    return Err(error("a truncated listing has no continuation token"));
                }
            } else {
                return Ok(objects);
            }
        }
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: &[u8],
    ) -> ServiceResult<Vec<u8>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("the system clock is before 1970"))?
            .as_secs();
        let (amz_date, date) = amz_timestamp(now);
        let payload_hash = sha256_hex(body);
        let mut query: Vec<(String, String)> = query
            .iter()
            .map(|(name, value)| (uri_encode(name, true), uri_encode(value, true)))
            .collect();
        query.sort();
        let query_string = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let authorization =
            self.authorization(method, path, &query_string, &payload_hash, &amz_date, &date);
        let url = if query_string.is_empty() {
            format!("{}{path}", self.endpoint)
        } else {
            format!("{}{path}?{query_string}", self.endpoint)
        };
        let request = self
            .agent
            .request(method, &url)
            .set("x-amz-date", &amz_date)
            .set("x-amz-content-sha256", &payload_hash)
            .set("Authorization", &authorization);
        let result = if body.is_empty() && method != "PUT" {
            request.call()
        } else {
            request.send_bytes(body)
        };
        match result {
            Ok(response) => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|io| error(format!("{method} {path}: {io}")))?;
                Ok(bytes)
            }
            Err(ureq::Error::Status(status, response)) => {
                let text = response.into_string().unwrap_or_default();
                let code = element(&text, "Code").unwrap_or("unknown");
                Err(error(format!(
                    "S3 {method} {path} failed with {status} {code}"
                )))
            }
            Err(transport) => Err(error(format!("S3 {method} {path}: {transport}"))),
        }
    }

    fn authorization(
        &self,
        method: &str,
        path: &str,
        query_string: &str,
        payload_hash: &str,
        amz_date: &str,
        date: &str,
    ) -> String {
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{method}\n{path}\n{query_string}\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n\n{signed_headers}\n{payload_hash}",
            host = self.host,
        );
        let scope = format!("{date}/{REGION}/s3/aws4_request");
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        let key = hmac(format!("AWS4{}", self.secret_key).as_bytes(), date);
        let key = hmac(&key, REGION);
        let key = hmac(&key, "s3");
        let key = hmac(&key, "aws4_request");
        let signature = hex(&hmac(&key, &string_to_sign));
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        )
    }
}

fn hmac(key: &[u8], message: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn object_path(bucket: &str, key: &str) -> String {
    format!("/{bucket}/{}", uri_encode(key, false))
}

/// S3's URI encoding: everything but unreserved characters is
/// percent-encoded; `/` too when `encode_slash`.
fn uri_encode(text: &str, encode_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            b'/' if !encode_slash => encoded.push('/'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// `(YYYYMMDDTHHMMSSZ, YYYYMMDD)` for a Unix time.
fn amz_timestamp(seconds: u64) -> (String, String) {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let date = format!("{year:04}{month:02}{day:02}");
    let time = format!(
        "{:02}{:02}{:02}",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60
    );
    (format!("{date}T{time}Z"), date)
}

/// The text of the first `<name>` element.
fn element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    elements(xml, name).next()
}

/// The text of every `<name>` element, in order (no nesting of the same
/// name, which S3 listings never use).
fn elements<'a>(xml: &'a str, name: &str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut rest = xml;
    std::iter::from_fn(move || {
        let start = rest.find(&open)? + open.len();
        let end = rest[start..].find(&close)? + start;
        let found = &rest[start..end];
        rest = &rest[end + close.len()..];
        Some(found)
    })
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_follow_the_sigv4_format() {
        assert_eq!(
            amz_timestamp(0),
            ("19700101T000000Z".into(), "19700101".into())
        );
        // 2013-05-24T00:00:00Z, the date of AWS's documented examples.
        assert_eq!(
            amz_timestamp(1_369_353_600),
            ("20130524T000000Z".into(), "20130524".into())
        );
        assert_eq!(amz_timestamp(951_825_599).0, "20000229T115959Z");
    }

    #[test]
    fn signatures_match_the_aws_documented_examples() {
        // "GET Bucket Lifecycle" and "Get Bucket (List Objects)" from the
        // AWS Signature Version 4 header-authentication examples for S3.
        let client = S3Client::new(
            "https://examplebucket.s3.amazonaws.com",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let sign = |query: &str| {
            client.authorization("GET", "/", query, empty, "20130524T000000Z", "20130524")
        };
        assert_eq!(
            sign("lifecycle="),
            concat!(
                "AWS4-HMAC-SHA256 ",
                "Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, ",
                "SignedHeaders=host;x-amz-content-sha256;x-amz-date, ",
                "Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
            )
        );
        assert!(sign("max-keys=2&prefix=J").ends_with(
            "Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        ));
    }

    #[test]
    fn keys_and_queries_are_uri_encoded() {
        assert_eq!(uri_encode("a b/c~d", false), "a%20b/c~d");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(
            object_path("bucket", "dir/x y.zip"),
            "/bucket/dir/x%20y.zip"
        );
    }

    #[test]
    fn listings_are_parsed() {
        let xml = "<ListBucketResult><IsTruncated>false</IsTruncated>\
            <Contents><Key>a&amp;b.zip</Key><Size>12</Size></Contents>\
            <Contents><Key>c.zip</Key><Size>3</Size></Contents></ListBucketResult>";
        let keys: Vec<_> = elements(xml, "Contents")
            .map(|item| unescape_xml(element(item, "Key").unwrap()))
            .collect();
        assert_eq!(keys, ["a&b.zip", "c.zip"]);
        assert_eq!(element(xml, "IsTruncated"), Some("false"));
    }
}
